use crate::{
    Result,
    database::{self, Candidate, Entry},
    error, normalize,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct General {
    pub suggestions: bool,
    pub history: bool,
    #[serde(default = "default_quick_lookup")]
    pub quick_lookup: bool,
    pub launch_at_startup: bool,
    pub close_to_tray: bool,
    pub release_minutes: u32,
}
fn default_quick_lookup() -> bool { true }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dictionary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub examples: String,
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub version: u32,
    pub general: General,
    pub dictionaries: Vec<Dictionary>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 2,
            general: General {
                suggestions: true,
                history: true,
                quick_lookup: true,
                launch_at_startup: false,
                close_to_tray: true,
                release_minutes: 5,
            },
            dictionaries: vec![],
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self.version != 2 {
            return Err("设置文件版本不受支持。".into());
        }
        let mut ids = HashSet::new();
        for d in &self.dictionaries {
            if d.id.is_empty()
                || !ids.insert(&d.id)
                || d.name.trim().is_empty()
                || !["expanded", "collapsed"].contains(&d.examples.as_str())
            {
                return Err("词典设置无效。".into());
            }
            let path = Path::new(&d.path);
            if path.is_absolute()
                || path.components().any(|p| {
                    matches!(
                        p,
                        std::path::Component::ParentDir | std::path::Component::Prefix(_)
                    )
                })
                || !d.path.starts_with("dictionaries/")
            {
                return Err("词库路径必须位于 dictionaries 目录。".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize)]
pub struct Library {
    #[serde(flatten)]
    pub config: Dictionary,
    pub entries: i64,
    pub available: bool,
    pub quality: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Record {
    pub word: String,
    pub language: String,
    pub gloss: String,
    pub pronunciation: String,
}
#[derive(Serialize)]
pub struct Snapshot {
    pub settings: Settings,
    pub libraries: Vec<Library>,
    pub saved: Vec<Record>,
    pub history: Vec<Record>,
    pub data_directory: String,
}

pub struct Store {
    pub root: PathBuf,
    pub settings: Settings,
    users: Connection,
    validated: RefCell<HashMap<String, (u64, SystemTime, Result<database::Metadata>)>>,
}
impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("dictionaries")).map_err(error)?;
        fs::create_dir_all(root.join("data")).map_err(error)?;
        let settings_path = root.join("dictlite.settings.json");
        let settings: Settings = if settings_path.exists() {
            serde_json::from_slice(&fs::read(&settings_path).map_err(error)?)
                .map_err(|e| format!("设置文件无法读取，请修复后重试：{e}"))?
        } else {
            Settings::default()
        };
        settings.validate()?;
        let users = Connection::open(root.join("data/user.sqlite")).map_err(error)?;
        users.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE IF NOT EXISTS records(kind TEXT NOT NULL,identity TEXT NOT NULL,word TEXT NOT NULL,language TEXT NOT NULL,gloss TEXT NOT NULL,pronunciation TEXT NOT NULL,updated INTEGER NOT NULL,PRIMARY KEY(kind,identity));").map_err(error)?;
        let s = Self {
            root,
            settings,
            users,
            validated: RefCell::new(HashMap::new()),
        };
        if !settings_path.exists() {
            s.write_settings(&s.settings)?;
        }
        Ok(s)
    }
    fn write_settings(&self, value: &Settings) -> Result<()> {
        value.validate()?;
        let target = self.root.join("dictlite.settings.json");
        let temp = self.root.join("dictlite.settings.json.tmp");
        fs::write(&temp, serde_json::to_vec_pretty(value).map_err(error)?).map_err(error)?;
        // Windows rename cannot replace an existing destination. ReplaceFileW preserves the old file on failure.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            unsafe extern "system" {
                fn ReplaceFileW(
                    a: *const u16,
                    b: *const u16,
                    c: *const u16,
                    d: u32,
                    e: *const std::ffi::c_void,
                    f: *const std::ffi::c_void,
                ) -> i32;
            }
            if target.exists() {
                let a: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
                let b: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
                if unsafe {
                    ReplaceFileW(
                        a.as_ptr(),
                        b.as_ptr(),
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                } == 0
                {
                    return Err(error(std::io::Error::last_os_error()));
                }
            } else {
                fs::rename(temp, target).map_err(error)?;
            }
        }
        #[cfg(not(windows))]
        fs::rename(temp, target).map_err(error)?;
        Ok(())
    }
    pub fn save_settings(&mut self, settings: Settings) -> Result<()> {
        self.write_settings(&settings)?;
        self.settings = settings;
        Ok(())
    }
    pub fn libraries(&self) -> Vec<Library> {
        self.settings
            .dictionaries
            .iter()
            .map(|d| {
                let path = self.root.join(&d.path);
                let signature = fs::metadata(&path).and_then(|m| Ok((m.len(), m.modified()?)));
                let result = match signature {
                    Ok((size, modified)) => {
                        let mut cache = self.validated.borrow_mut();
                        if !cache
                            .get(&d.path)
                            .is_some_and(|v| v.0 == size && v.1 == modified)
                        {
                            cache.insert(
                                d.path.clone(),
                                (size, modified, database::metadata(&path)),
                            );
                        }
                        cache[&d.path].2.clone()
                    }
                    Err(e) => Err(error(e)),
                };
                match result {
                    Ok(m) if m.id == d.id => Library {
                        config: d.clone(),
                        entries: m.entries,
                        available: true,
                        quality: Some(m.quality),
                        error: None,
                    },
                    Ok(_) => Library {
                        config: d.clone(),
                        entries: 0,
                        available: false,
                        quality: None,
                        error: Some("词库身份与登记不一致".into()),
                    },
                    Err(e) => Library {
                        config: d.clone(),
                        entries: 0,
                        available: false,
                        quality: None,
                        error: Some(e),
                    },
                }
            })
            .collect()
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            settings: self.settings.clone(),
            libraries: self.libraries(),
            saved: self.records("saved")?,
            history: self.records("history")?,
            data_directory: self.root.display().to_string(),
        })
    }
    pub fn search(&self, q: &str, mode: &str) -> Result<Vec<Candidate>> {
        if !["auto", "en", "zh"].contains(&mode) {
            return Err("查询方向无效。".into());
        }
        if q.chars().count() > 256 {
            return Err("查询内容过长。".into());
        }
        let mut hits = vec![];
        for (order, d) in self
            .settings
            .dictionaries
            .iter()
            .enumerate()
            .filter(|(_, d)| d.enabled)
        {
            if let Ok(c) = database::open_read(&self.root.join(&d.path)) {
                if let Ok(rows) = database::search(&c, &d.id, q, mode) {
                    for row in rows {
                        hits.push((order, row));
                    }
                }
            }
        }
        hits.sort_by_key(|(order, c)| {
            (
                c.rank,
                c.relevance,
                *order,
                c.word.chars().count(),
                c.entry_id,
            )
        });
        let mut keys = HashSet::new();
        Ok(hits
            .into_iter()
            .filter_map(|(_, c)| {
                if keys.insert((normalize(&c.word), c.language.clone())) {
                    Some(c)
                } else {
                    None
                }
            })
            .take(8)
            .collect())
    }
    pub fn sources(&self, word: &str, lang: &str) -> Result<Vec<Candidate>> {
        let mut out = vec![];
        for d in self.settings.dictionaries.iter().filter(|d| d.enabled) {
            if let Ok(c) = database::open_read(&self.root.join(&d.path)) {
                let rows = database::search(&c, &d.id, word, lang).unwrap_or_default();
                for row in rows.into_iter().filter(|r| r.rank == 0) {
                    out.push(row);
                }
            }
        }
        Ok(out)
    }
    pub fn entry(&self, dictionary: &str, id: i64) -> Result<Entry> {
        let d = self
            .settings
            .dictionaries
            .iter()
            .find(|d| d.id == dictionary && d.enabled)
            .ok_or("词典未启用")?;
        database::detail(
            &database::open_read(&self.root.join(&d.path))?,
            dictionary,
            id,
        )
    }
    pub fn record_query(&self, item: &Record) -> Result<()> {
        if self.settings.general.history {
            self.put("history", item)?;
            self.users.execute("DELETE FROM records WHERE kind='history' AND identity NOT IN (SELECT identity FROM records WHERE kind='history' ORDER BY updated DESC,rowid DESC LIMIT 50)",[]).map_err(error)?;
        }
        Ok(())
    }
    fn put(&self, kind: &str, item: &Record) -> Result<()> {
        let identity = format!("{}:{}", item.language, normalize(&item.word));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        self.users.execute("INSERT INTO records(kind,identity,word,language,gloss,pronunciation,updated) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(kind,identity) DO UPDATE SET word=excluded.word,gloss=excluded.gloss,pronunciation=excluded.pronunciation,updated=excluded.updated",params![kind,identity,item.word,item.language,item.gloss,item.pronunciation,now]).map_err(error)?;
        Ok(())
    }
    pub fn toggle_saved(&self, item: &Record) -> Result<bool> {
        let identity = format!("{}:{}", item.language, normalize(&item.word));
        let removed = self
            .users
            .execute(
                "DELETE FROM records WHERE kind='saved' AND identity=?1",
                [&identity],
            )
            .map_err(error)?;
        if removed == 0 {
            self.put("saved", item)?;
        }
        Ok(removed == 0)
    }
    pub fn records(&self, kind: &str) -> Result<Vec<Record>> {
        self.users.prepare("SELECT word,language,gloss,pronunciation FROM records WHERE kind=?1 ORDER BY updated DESC,rowid DESC").map_err(error)?.query_map([kind],|r|Ok(Record{word:r.get(0)?,language:r.get(1)?,gloss:r.get(2)?,pronunciation:r.get(3)?})).map_err(error)?.collect::<std::result::Result<Vec<_>,_>>().map_err(error)
    }
    pub fn clear_history(&self) -> Result<()> {
        self.users
            .execute("DELETE FROM records WHERE kind='history'", [])
            .map_err(error)?;
        Ok(())
    }
    pub fn import(&mut self, path: &Path, accept_review: bool) -> Result<database::Metadata> {
        let meta = database::validate(path)?;
        if meta.quality == "needs_review" && !accept_review {
            return Err("词库存在内容缺失，请检查转换报告后再导入。".into());
        }
        if self.settings.dictionaries.iter().any(|d| d.id == meta.id) {
            return Err("该词典已存在，未覆盖原词典。".into());
        }
        let name = format!("{}.sqlite", uuid::Uuid::new_v4());
        let relative = format!("dictionaries/{name}");
        let target = self.root.join(&relative);
        fs::copy(path, &target).map_err(error)?;
        if let Err(e) = database::validate(&target) {
            let _ = fs::remove_file(&target);
            return Err(e);
        }
        let mut settings = self.settings.clone();
        settings.dictionaries.push(Dictionary {
            id: meta.id.clone(),
            name: meta.name.clone(),
            enabled: true,
            examples: "collapsed".into(),
            path: relative,
        });
        if let Err(e) = self.save_settings(settings) {
            let _ = fs::remove_file(&target);
            return Err(e);
        }
        Ok(meta)
    }
}
