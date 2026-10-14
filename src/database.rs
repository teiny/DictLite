use crate::{Result, error, normalize};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

pub const SCHEMA: &str = r#"
PRAGMA foreign_keys=ON;
CREATE TABLE dictionary_meta(format TEXT NOT NULL, schema_version INTEGER NOT NULL, normalization_version INTEGER NOT NULL,
 dictionary_id TEXT NOT NULL, name TEXT NOT NULL, content_version TEXT NOT NULL DEFAULT '', source_format TEXT NOT NULL,
 source_hash TEXT NOT NULL, converter_version TEXT NOT NULL, profile TEXT NOT NULL, created_at TEXT NOT NULL,
 entry_count INTEGER NOT NULL DEFAULT 0, alias_count INTEGER NOT NULL DEFAULT 0, quality_status TEXT NOT NULL);
CREATE TABLE entries(entry_id INTEGER PRIMARY KEY, headword TEXT NOT NULL, normalized_headword TEXT NOT NULL,
 language TEXT NOT NULL, gloss TEXT NOT NULL DEFAULT '', body_text TEXT NOT NULL, parse_status TEXT NOT NULL, source_record INTEGER NOT NULL);
CREATE TABLE pronunciations(entry_id INTEGER NOT NULL REFERENCES entries, kind TEXT NOT NULL, text TEXT NOT NULL, sort_order INTEGER NOT NULL);
CREATE TABLE senses(sense_id INTEGER PRIMARY KEY, entry_id INTEGER NOT NULL REFERENCES entries, part_of_speech TEXT NOT NULL DEFAULT '',
 translation TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', sort_order INTEGER NOT NULL);
CREATE TABLE examples(example_id INTEGER PRIMARY KEY, entry_id INTEGER NOT NULL REFERENCES entries, sense_id INTEGER REFERENCES senses,
 source_text TEXT NOT NULL, translation TEXT NOT NULL DEFAULT '', sort_order INTEGER NOT NULL);
CREATE TABLE aliases(alias_id INTEGER PRIMARY KEY, headword TEXT NOT NULL, normalized_headword TEXT NOT NULL, language TEXT NOT NULL,
 target_entry_id INTEGER NOT NULL REFERENCES entries);
CREATE TABLE entry_links(entry_id INTEGER NOT NULL REFERENCES entries, target_headword TEXT NOT NULL, target_entry_id INTEGER REFERENCES entries,
 relation TEXT NOT NULL, sort_order INTEGER NOT NULL);
CREATE VIRTUAL TABLE search_terms USING fts5(terms,content='');
CREATE INDEX entries_headword ON entries(normalized_headword,language);
CREATE INDEX aliases_headword ON aliases(normalized_headword,language);
CREATE INDEX pronunciations_entry ON pronunciations(entry_id);
CREATE INDEX senses_entry ON senses(entry_id);
CREATE INDEX examples_entry ON examples(entry_id);
CREATE INDEX links_entry ON entry_links(entry_id);
"#;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub id: String,
    pub name: String,
    pub entries: i64,
    pub aliases: i64,
    pub quality: String,
    pub profile: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub word: String,
    pub language: String,
    pub gloss: String,
    pub dictionary: String,
    pub entry_id: i64,
    pub rank: i64,
    pub relevance: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pronunciation {
    pub kind: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sense {
    pub pos: String,
    pub translation: String,
    pub note: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Example {
    pub text: String,
    pub translation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub word: String,
    pub language: String,
    pub gloss: String,
    pub body: String,
    pub parse_status: String,
    pub pronunciations: Vec<Pronunciation>,
    pub senses: Vec<Sense>,
    pub examples: Vec<Example>,
    pub related: Vec<String>,
    pub dictionary: String,
}

pub fn open_read(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(error)?;
    conn.pragma_update(None, "query_only", true)
        .map_err(error)?;
    Ok(conn)
}
pub fn metadata(path: &Path) -> Result<Metadata> {
    read_metadata(&open_read(path)?)
}
fn read_metadata(conn: &Connection) -> Result<Metadata> {
    if conn
        .query_row("SELECT count(*) FROM dictionary_meta", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(error)?
        != 1
    {
        return Err("词库元信息无效。".into());
    }
    let (format, version, normalization, meta) = conn.query_row(
        "SELECT format,schema_version,normalization_version,dictionary_id,name,entry_count,alias_count,quality_status,profile FROM dictionary_meta", [], |r| {
        Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,Metadata{id:r.get(3)?,name:r.get(4)?,entries:r.get(5)?,aliases:r.get(6)?,quality:r.get(7)?,profile:r.get(8)?}))
    }).map_err(|_| "这不是有效的 DictLite 词库。".to_string())?;
    if format != "DictLite" || version != 1 || normalization != 1 {
        return Err("词库格式或版本不受支持。".into());
    }
    if meta.id.is_empty()
        || meta.name.trim().is_empty()
        || !["complete", "needs_review"].contains(&meta.quality.as_str())
    {
        return Err("词库标识或质量状态无效。".into());
    }
    Ok(meta)
}
pub fn validate(path: &Path) -> Result<Metadata> {
    let conn = open_read(path)?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(error)?;
    if integrity != "ok" {
        return Err("词库完整性检查未通过。".into());
    }
    let meta = read_metadata(&conn)?;
    let count: i64 = conn
        .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
        .map_err(error)?;
    let aliases: i64 = conn
        .query_row("SELECT count(*) FROM aliases", [], |r| r.get(0))
        .map_err(error)?;
    if count == 0 || count != meta.entries || aliases != meta.aliases {
        return Err("词条数量与词库信息不一致，或词库为空。".into());
    }
    for sql in [
        "SELECT entry_id,headword,normalized_headword,language,gloss,body_text,parse_status,source_record FROM entries LIMIT 1",
        "SELECT entry_id,kind,text,sort_order FROM pronunciations LIMIT 1",
        "SELECT sense_id,entry_id,part_of_speech,translation,note,sort_order FROM senses LIMIT 1",
        "SELECT example_id,entry_id,sense_id,source_text,translation,sort_order FROM examples LIMIT 1",
        "SELECT alias_id,headword,normalized_headword,language,target_entry_id FROM aliases LIMIT 1",
        "SELECT entry_id,target_headword,target_entry_id,relation,sort_order FROM entry_links LIMIT 1",
        "SELECT rowid,terms FROM search_terms LIMIT 1",
    ] {
        conn.prepare(sql)
            .map_err(|_| "词库缺少必要的表或字段。".to_string())?;
    }
    if conn
        .prepare("PRAGMA foreign_key_check")
        .map_err(error)?
        .query([])
        .map_err(error)?
        .next()
        .map_err(error)?
        .is_some()
    {
        return Err("词库包含无效引用。".into());
    }
    let invalid: i64 = conn.query_row("SELECT count(*) FROM entries WHERE trim(headword)='' OR trim(body_text)='' OR language NOT IN ('en','zh','und') OR parse_status NOT IN ('structured','fallback')",[],|r|r.get(0)).map_err(error)?;
    if invalid > 0 {
        return Err("词库中存在无效词条。".into());
    }
    let invalid_examples: i64 = conn.query_row("SELECT count(*) FROM examples x JOIN senses s ON x.sense_id=s.sense_id WHERE x.entry_id<>s.entry_id",[],|r|r.get(0)).map_err(error)?;
    if invalid_examples > 0 {
        return Err("例句和义项归属不一致。".into());
    }
    for index in [
        "entries_headword",
        "aliases_headword",
        "pronunciations_entry",
        "senses_entry",
        "examples_entry",
        "links_entry",
    ] {
        if conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name=?1",
                [index],
                |r| r.get::<_, i64>(0),
            )
            .map_err(error)?
            != 1
        {
            return Err(format!("词库缺少查询索引：{index}"));
        }
    }
    Ok(meta)
}

pub fn search(
    conn: &Connection,
    dictionary: &str,
    query: &str,
    mode: &str,
) -> Result<Vec<Candidate>> {
    let q = normalize(query);
    if q.is_empty() {
        return Ok(vec![]);
    }
    let upper = format!("{q}\u{10ffff}");
    let probe = terms(&q)
        .into_iter()
        .filter(|term| term.chars().count() <= 3)
        .max_by_key(|x| x.chars().count())
        .unwrap_or_else(|| q.clone());
    let probe = format!("\"{}\"", probe.replace('"', "\"\""));
    let sql = "WITH hits AS (
      SELECT entry_id,CASE WHEN normalized_headword=?1 THEN 0 ELSE 1 END rank FROM entries WHERE normalized_headword>=?1 AND normalized_headword<?2
      UNION ALL SELECT target_entry_id,CASE WHEN normalized_headword=?1 THEN 0 ELSE 1 END FROM aliases WHERE normalized_headword>=?1 AND normalized_headword<?2
      UNION ALL SELECT e.entry_id,CASE WHEN instr(e.normalized_headword,?1)>0 THEN 2 ELSE 3 END FROM search_terms t JOIN entries e ON e.entry_id=t.rowid
        WHERE t.terms MATCH ?3 AND (instr(e.normalized_headword,?1)>0 OR instr(lower(e.body_text),?1)>0 OR instr(lower(e.gloss),?1)>0)
    ) SELECT e.entry_id,e.headword,e.language,e.gloss,min(h.rank),
      CASE WHEN min(h.rank)<3 THEN 0 WHEN e.gloss=?1 OR (substr(e.gloss,1,length(?1))=?1 AND substr(e.gloss,length(?1)+1,1) IN (',','，',';','；',' ',char(10))) THEN 0
      WHEN instr(e.gloss,', '||?1)>0 OR instr(e.gloss,'；'||?1)>0 OR instr(e.gloss,'，'||?1)>0 THEN 1 ELSE 2 END relevance
      FROM hits h JOIN entries e ON e.entry_id=h.entry_id
      WHERE (?4='auto' OR e.language=?4 OR e.language='und') GROUP BY e.entry_id ORDER BY min(h.rank),relevance,length(e.headword),e.entry_id LIMIT 24";
    let mut stmt = conn.prepare(sql).map_err(error)?;
    stmt.query_map(params![q, upper, probe, mode], |r| {
        Ok(Candidate {
            entry_id: r.get(0)?,
            word: r.get(1)?,
            language: r.get(2)?,
            gloss: r.get(3)?,
            rank: r.get(4)?,
            relevance: r.get(5)?,
            dictionary: dictionary.into(),
        })
    })
    .map_err(error)?
    .collect::<std::result::Result<Vec<_>, _>>()
    .map_err(error)
}
pub fn detail(conn: &Connection, dictionary: &str, id: i64) -> Result<Entry> {
    let mut item=conn.query_row("SELECT entry_id,headword,language,gloss,body_text,parse_status FROM entries WHERE entry_id=?1",[id],|r|Ok(Entry{id:r.get(0)?,word:r.get(1)?,language:r.get(2)?,gloss:r.get(3)?,body:r.get(4)?,parse_status:r.get(5)?,pronunciations:vec![],senses:vec![],examples:vec![],related:vec![],dictionary:dictionary.into()})).map_err(error)?;
    item.pronunciations = conn
        .prepare("SELECT kind,text FROM pronunciations WHERE entry_id=?1 ORDER BY sort_order")
        .map_err(error)?
        .query_map([id], |r| {
            Ok(Pronunciation {
                kind: r.get(0)?,
                text: r.get(1)?,
            })
        })
        .map_err(error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(error)?;
    item.senses=conn.prepare("SELECT part_of_speech,translation,note FROM senses WHERE entry_id=?1 ORDER BY sort_order").map_err(error)?.query_map([id],|r|Ok(Sense{pos:r.get(0)?,translation:r.get(1)?,note:r.get(2)?})).map_err(error)?.collect::<std::result::Result<Vec<_>,_>>().map_err(error)?;
    item.examples = conn
        .prepare(
            "SELECT source_text,translation FROM examples WHERE entry_id=?1 ORDER BY sort_order",
        )
        .map_err(error)?
        .query_map([id], |r| {
            Ok(Example {
                text: r.get(0)?,
                translation: r.get(1)?,
            })
        })
        .map_err(error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(error)?;
    item.related=conn.prepare("SELECT target_headword FROM entry_links WHERE entry_id=?1 ORDER BY sort_order LIMIT 24").map_err(error)?.query_map([id],|r|r.get(0)).map_err(error)?.collect::<std::result::Result<Vec<_>,_>>().map_err(error)?;
    Ok(item)
}

pub fn terms(text: &str) -> BTreeSet<String> {
    let lower = normalize(text);
    let mut result = BTreeSet::new();
    for token in lower.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-') {
        if token.is_empty() {
            continue;
        }
        let chars: Vec<char> = token.chars().collect();
        if chars.iter().any(|c| ('\u{3400}'..='\u{9fff}').contains(c)) {
            for c in &chars {
                result.insert(c.to_string());
            }
            for pair in chars.windows(2) {
                result.insert(pair.iter().collect());
            }
        } else {
            result.insert(token.to_string());
            if chars.len() >= 3 {
                for gram in chars.windows(3) {
                    result.insert(gram.iter().collect());
                }
            }
        }
    }
    result
}
pub fn index_text(conn: &Connection, id: i64, text: &str) -> Result<()> {
    let value = terms(text).into_iter().collect::<Vec<_>>().join(" ");
    conn.execute(
        "INSERT INTO search_terms(rowid,terms) VALUES(?1,?2)",
        params![id, value],
    )
    .map_err(error)?;
    Ok(())
}
