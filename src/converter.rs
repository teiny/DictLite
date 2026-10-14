use crate::{Result, database, error, language, normalize};
use rusqlite::{Connection, params};
use scraper::{Html, Selector, node::Node};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct Options {
    pub input: PathBuf,
    pub output: PathBuf,
    pub report: PathBuf,
    pub task_id: String,
    pub dictionary_id: Option<String>,
    pub profile: String,
}
#[derive(Serialize, Deserialize, Debug)]
pub struct Report {
    pub protocol_version: u32,
    pub task_id: String,
    pub source_hash: String,
    pub converter_version: String,
    pub profile: String,
    pub quality_status: String,
    pub source_records: u64,
    pub entries: u64,
    pub aliases: u64,
    pub structured: u64,
    pub fallback: u64,
    pub skipped: u64,
    pub media_removed: u64,
    pub warnings: u64,
    pub issues: Vec<String>,
}
fn issue(report: &mut Report, message: String) {
    report.warnings += 1;
    if report.issues.len() < 100 {
        report.issues.push(message);
    }
}
fn strip_styles(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '`' {
            let mut look = chars.clone();
            let mut digits = 0;
            while look.peek().is_some_and(|c| c.is_ascii_digit()) {
                look.next();
                digits += 1;
            }
            if digits > 0 && digits < 8 && look.next() == Some('`') {
                for _ in 0..=digits {
                    chars.next();
                }
                continue;
            }
        }
        result.push(c);
    }
    result
}
pub fn clean_html(html: &str) -> (String, u64) {
    let html = strip_styles(html);
    let doc = Html::parse_fragment(&html);
    let mut text = String::new();
    let mut media = 0;
    for node in doc.tree.root().descendants() {
        let blocked = node.ancestors().any(|n| match n.value() {
            Node::Element(el) => {
                [
                    "script", "style", "audio", "video", "iframe", "object", "svg", "noscript",
                ]
                .contains(&el.name())
                    || (el.name() == "a"
                        && el.attr("href").is_some_and(|h| {
                            h.starts_with("sound:") || h.starts_with("javascript:")
                        }))
            }
            _ => false,
        });
        if blocked {
            continue;
        }
        match node.value() {
            Node::Element(el) => {
                if ["img", "audio", "video", "source", "iframe", "object"].contains(&el.name()) {
                    media += 1;
                }
                if [
                    "p", "div", "br", "li", "tr", "td", "h1", "h2", "h3", "section",
                ]
                .contains(&el.name())
                    && !text.ends_with('\n')
                {
                    text.push('\n');
                }
            }
            Node::Text(t) => {
                text.push_str(t);
            }
            _ => {}
        }
    }
    let out = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (out, media)
}
fn select_text(doc: &Html, selector: &str) -> Vec<String> {
    let selector = Selector::parse(selector).unwrap();
    doc.select(&selector)
        .map(|e| clean_html(&e.html()).0)
        .filter(|s| !s.trim().is_empty())
        .collect()
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err("cancelled".into())
    } else {
        Ok(())
    }
}
pub fn concise_fields(
    body: &str,
    head: &str,
) -> (Vec<database::Sense>, Vec<database::Pronunciation>) {
    let mut senses: Vec<database::Sense> = vec![];
    let mut pronunciation = vec![];
    for line in body.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if line == head {
            continue;
        }
        if line.starts_with('[') {
            if let Some(end) = line.find(']') {
                let phonetic = &line[1..end];
                if !phonetic.is_empty()
                    && !phonetic
                        .chars()
                        .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
                    && line[end + 1..].trim().starts_with("-K")
                {
                    pronunciation.push(database::Pronunciation {
                        kind: "other".into(),
                        text: phonetic.into(),
                    });
                    continue;
                }
            }
        }
        if let Some((pos, text)) = line.split_once(' ') {
            if [
                "n.", "a.", "adj.", "v.", "vt.", "vi.", "adv.", "prep.", "pron.", "conj.",
                "interj.", "num.", "aux.", "abbr.", "art.", "int.", "pl.",
            ]
            .contains(&pos)
                && !text.trim().is_empty()
            {
                senses.push(database::Sense {
                    pos: pos.into(),
                    translation: text.trim().into(),
                    note: String::new(),
                });
                continue;
            }
        }
        if line.starts_with('[') {
            if let Some(last) = senses.last_mut() {
                if !last.note.is_empty() {
                    last.note.push('\n');
                }
                last.note.push_str(line);
            }
        }
    }
    (senses, pronunciation)
}

pub fn convert(
    options: &Options,
    cancel: &AtomicBool,
    mut event: impl FnMut(Value),
) -> Result<Report> {
    if !["auto", "generic", "concise"].contains(&options.profile.as_str()) {
        return Err("当前版本支持 auto、generic 或 concise 模式。".into());
    }
    if !options.input.is_file() {
        return Err("找不到输入 MDX 文件。".into());
    }
    if options.output.exists() || options.report.exists() {
        return Err("输出文件已存在，未执行覆盖。".into());
    }
    let input = fs::canonicalize(&options.input).map_err(error)?;
    let out_parent = options
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let report_parent = options
        .report
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output_abs = fs::canonicalize(out_parent)
        .map_err(error)?
        .join(options.output.file_name().ok_or("输出路径无效")?);
    let report_abs = fs::canonicalize(report_parent)
        .map_err(error)?
        .join(options.report.file_name().ok_or("报告路径无效")?);
    if input == output_abs || input == report_abs || output_abs == report_abs {
        return Err("输入、词库输出与报告必须是不同文件。".into());
    }
    let temp = output_abs.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let temp_report = report_abs.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        check_cancel(cancel)?;
        event(json!({"type":"started","stage":"preflight"}));
        let mut input_file = fs::File::open(&input).map_err(error)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            check_cancel(cancel)?;
            let n = input_file.read(&mut buffer).map_err(error)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        let source = mdictlib::MdxFile::open(&input).map_err(|e| format!("无法解析 MDX：{e}"))?;
        let name = source
            .header()
            .title()
            .filter(|s| !s.trim().is_empty())
            .map(|s| clean_html(s).0)
            .unwrap_or_else(|| {
                input
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let concise = options.profile == "concise"
            || (options.profile == "auto" && name == "简明英汉字典增强版");
        let mut report = Report {
            protocol_version: 1,
            task_id: options.task_id.clone(),
            source_hash: format!("{:x}", hash.finalize()),
            converter_version: env!("CARGO_PKG_VERSION").into(),
            profile: if concise { "concise" } else { "generic" }.into(),
            quality_status: "complete".into(),
            source_records: 0,
            entries: 0,
            aliases: 0,
            structured: 0,
            fallback: 0,
            skipped: 0,
            media_removed: 0,
            warnings: 0,
            issues: vec![],
        };
        // The file is exclusively created by this task before SQLite opens it.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(error)?;
        let conn = Connection::open(&temp).map_err(error)?;
        // This is an unpublished, disposable database. Failure discards it, so durable per-batch journals are unnecessary.
        conn.execute_batch("PRAGMA journal_mode=OFF;PRAGMA synchronous=OFF;PRAGMA cache_size=-65536;PRAGMA temp_store=FILE;").map_err(error)?;
        conn.execute_batch(database::SCHEMA).map_err(error)?;
        conn.execute_batch("CREATE TABLE pending_alias(source_record INTEGER PRIMARY KEY,headword TEXT,target TEXT); BEGIN;").map_err(error)?;
        let id = options
            .dictionary_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let created = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(error)?;
        conn.execute("INSERT INTO dictionary_meta VALUES('DictLite',1,1,?1,?2,'','mdx',?3,?4,?5,?6,0,0,'complete')",params![id,name,report.source_hash,env!("CARGO_PKG_VERSION"),report.profile,created]).map_err(error)?;
        for source_entry in source.entries() {
            check_cancel(cancel)?;
            let record = source_entry.map_err(|e| format!("读取 MDX 词条失败：{e}"))?;
            report.source_records += 1;
            let head = record.key().trim();
            let raw = record.text();
            if let Some(target) = raw.trim().strip_prefix("@@@LINK=") {
                conn.execute(
                    "INSERT INTO pending_alias VALUES(?1,?2,?3)",
                    params![report.source_records, head, target.trim()],
                )
                .map_err(error)?;
                continue;
            }
            if head.is_empty() || raw.len() > 8 * 1024 * 1024 {
                report.skipped += 1;
                let ordinal = report.source_records;
                issue(
                    &mut report,
                    format!("记录 {ordinal} 词头为空或正文超过 8 MiB"),
                );
                continue;
            }
            let (body, removed) = clean_html(raw);
            report.media_removed += removed;
            if body.is_empty() {
                report.skipped += 1;
                issue(&mut report, format!("{head}：没有可读取的文字正文"));
                continue;
            }
            if body.contains('\u{fffd}')
                || body.chars().any(|c| ('\u{e000}'..='\u{f8ff}').contains(&c))
            {
                issue(&mut report, format!("{head}：含无法可靠恢复的字符"));
                report.quality_status = "needs_review".into();
            }
            let doc = Html::parse_fragment(
                if raw.contains("data-dictlite-") || raw.contains("entry://") {
                    raw
                } else {
                    ""
                },
            );
            // Only explicit structural markers are trusted. Arbitrary MDX templates remain readable text.
            let definitions = select_text(&doc, "[data-dictlite-definition]");
            let poses = select_text(&doc, "[data-dictlite-pos]");
            let (concise_senses, concise_pronunciations) = if concise {
                concise_fields(&body, head)
            } else {
                (vec![], vec![])
            };
            let structured = (!definitions.is_empty() && definitions.len() == poses.len())
                || !concise_senses.is_empty();
            let gloss = if !concise_senses.is_empty() {
                concise_senses
                    .iter()
                    .map(|s| s.translation.as_str())
                    .collect::<Vec<_>>()
                    .join("；")
            } else if structured {
                definitions.join("；")
            } else {
                body.lines()
                    .filter(|s| *s != head)
                    .next()
                    .unwrap_or(&body)
                    .chars()
                    .take(100)
                    .collect()
            };
            conn.execute("INSERT INTO entries(headword,normalized_headword,language,gloss,body_text,parse_status,source_record) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![head,normalize(head),language(head),gloss,body,if structured{"structured"}else{"fallback"},report.source_records]).map_err(error)?;
            let entry_id = conn.last_insert_rowid();
            report.entries += 1;
            if structured {
                report.structured += 1;
                for (i, (pos, translation)) in poses.iter().zip(definitions.iter()).enumerate() {
                    conn.execute("INSERT INTO senses(entry_id,part_of_speech,translation,note,sort_order) VALUES(?1,?2,?3,'',?4)",params![entry_id,pos,translation,i]).map_err(error)?;
                }
                for (i, s) in concise_senses.iter().enumerate() {
                    conn.execute("INSERT INTO senses(entry_id,part_of_speech,translation,note,sort_order) VALUES(?1,?2,?3,?4,?5)",params![entry_id,s.pos,s.translation,s.note,i]).map_err(error)?;
                }
            } else {
                report.fallback += 1;
            }
            for (i, p) in concise_pronunciations.iter().enumerate() {
                conn.execute(
                    "INSERT INTO pronunciations VALUES(?1,?2,?3,?4)",
                    params![entry_id, p.kind, p.text, i],
                )
                .map_err(error)?;
            }
            for (kind, selector) in [
                ("uk", "[data-dictlite-uk]"),
                ("us", "[data-dictlite-us]"),
                ("pinyin", "[data-dictlite-pinyin]"),
            ] {
                for (i, text) in select_text(&doc, selector).iter().enumerate() {
                    conn.execute(
                        "INSERT INTO pronunciations VALUES(?1,?2,?3,?4)",
                        params![entry_id, kind, text, i],
                    )
                    .map_err(error)?;
                }
            }
            for (i, e) in doc
                .select(&Selector::parse("[data-dictlite-example]").unwrap())
                .enumerate()
            {
                let text = clean_html(&e.inner_html()).0;
                let translation = e.value().attr("data-translation").unwrap_or("");
                conn.execute("INSERT INTO examples(entry_id,source_text,translation,sort_order) VALUES(?1,?2,?3,?4)",params![entry_id,text,translation,i]).map_err(error)?;
            }
            for (i, a) in doc
                .select(&Selector::parse("a[href]").unwrap())
                .filter_map(|a| {
                    a.value()
                        .attr("href")
                        .filter(|h| h.starts_with("entry://"))
                        .map(|h| {
                            h.trim_start_matches("entry://")
                                .trim_end_matches('/')
                                .to_string()
                        })
                })
                .enumerate()
            {
                conn.execute(
                    "INSERT INTO entry_links VALUES(?1,?2,NULL,'related',?3)",
                    params![entry_id, a, i],
                )
                .map_err(error)?;
            }
            database::index_text(&conn, entry_id, &format!("{head}\n{body}"))?;
            if report.source_records % 2000 == 0 {
                conn.execute_batch("COMMIT;BEGIN;").map_err(error)?;
                event(
                    json!({"type":"progress","stage":"extracting","processed":report.source_records,"total":source.len()}),
                );
            }
        }
        check_cancel(cancel)?;
        event(
            json!({"type":"progress","stage":"resolving","processed":report.source_records,"total":source.len()}),
        );
        for _ in 0..256 {
            check_cancel(cancel)?;
            let mut cursor = -1;
            let mut resolved = 0;
            loop {
                let mut stmt=conn.prepare("SELECT source_record,headword,target FROM pending_alias WHERE source_record>?1 ORDER BY source_record LIMIT 1000").map_err(error)?;
                let pending = stmt
                    .query_map([cursor], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(error)?;
                if pending.is_empty() {
                    break;
                }
                for (ordinal, head, target) in pending {
                    check_cancel(cancel)?;
                    cursor = ordinal;
                    let mut find=conn.prepare("SELECT entry_id FROM entries WHERE normalized_headword=?1 AND headword=?2 UNION SELECT target_entry_id FROM aliases WHERE normalized_headword=?1 AND headword=?2 LIMIT 2").map_err(error)?;
                    let ids = find
                        .query_map(params![normalize(&target), target], |r| r.get::<_, i64>(0))
                        .map_err(error)?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(error)?;
                    if ids.len() == 1 {
                        conn.execute("INSERT INTO aliases(headword,normalized_headword,language,target_entry_id) VALUES(?1,?2,?3,?4)",params![head,normalize(&head),language(&head),ids[0]]).map_err(error)?;
                        conn.execute(
                            "DELETE FROM pending_alias WHERE source_record=?1",
                            [ordinal],
                        )
                        .map_err(error)?;
                        report.aliases += 1;
                        resolved += 1;
                    }
                }
            }
            if resolved == 0 {
                break;
            }
        }
        let unresolved: u64 = conn
            .query_row("SELECT count(*) FROM pending_alias", [], |r| r.get(0))
            .map_err(error)?;
        report.skipped += unresolved;
        if unresolved > 0 {
            issue(
                &mut report,
                format!("{unresolved} 个跳转未能唯一解析，已保留在转换报告统计中"),
            );
        }
        if report.entries == 0 {
            return Err("词库没有可用文字词条，未生成输出。".into());
        }
        if report.skipped > 0 {
            report.quality_status = "needs_review".into();
        }
        if report.fallback > 0 {
            issue(
                &mut report,
                "部分词条以完整文字正文保存；当前未适配原词典的结构化模板。".into(),
            );
        }
        conn.execute(
            "UPDATE dictionary_meta SET entry_count=?1,alias_count=?2,quality_status=?3",
            params![report.entries, report.aliases, report.quality_status],
        )
        .map_err(error)?;
        conn.execute_batch("DROP TABLE pending_alias;COMMIT;INSERT INTO search_terms(search_terms) VALUES('optimize');PRAGMA optimize;").map_err(error)?;
        drop(conn);
        check_cancel(cancel)?;
        event(json!({"type":"progress","stage":"validating"}));
        database::validate(&temp)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_report)
            .map_err(error)?;
        fs::write(
            &temp_report,
            serde_json::to_vec_pretty(&report).map_err(error)?,
        )
        .map_err(error)?;
        check_cancel(cancel)?;
        // Hard links publish without overwriting a file created by another process in the meantime.
        fs::hard_link(&temp, &output_abs).map_err(error)?;
        if let Err(e) = fs::hard_link(&temp_report, &report_abs) {
            let _ = fs::remove_file(&output_abs);
            return Err(error(e));
        }
        event(
            json!({"type":"finished","status":"succeeded","quality_status":report.quality_status,"output":output_abs,"report":report_abs,"entries":report.entries,"aliases":report.aliases,"warnings":report.warnings}),
        );
        Ok(report)
    })();
    let _ = fs::remove_file(temp);
    let _ = fs::remove_file(temp_report);
    result
}
