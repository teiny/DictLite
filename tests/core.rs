use dictlite::{
    converter::{self, Options},
    database,
    store::{Record, Store},
};
use std::{path::Path, sync::atomic::AtomicBool};

fn source(path: &Path, extra: bool) {
    let mut builder = mdictlib::MdxBuilder::new();
    builder.header_attribute("Title", "测试词典").unwrap();
    builder.add_entry("light","<div><span data-dictlite-uk>laɪt</span><span data-dictlite-pos>n.</span><span data-dictlite-definition>光；光线</span><p data-dictlite-example data-translation='让光进来。'>Let the light in.</p><img src='picture.png'><a href='sound:light.mp3'>播放</a><script>bad_script()</script></div>").unwrap();
    builder
        .add_entry("bank", "<p>n. 银行；河岸</p><p>v. 存入银行</p>")
        .unwrap();
    builder.add_entry("银行", "<p>bank，金融机构</p>").unwrap();
    builder.add_entry("光", "<p>light；ray</p>").unwrap();
    builder.add_entry("shine", "@@@LINK=light").unwrap();
    if extra {
        builder.add_entry("broken", "<img src='scan.png'>").unwrap();
        builder.add_entry("loop", "@@@LINK=loop").unwrap();
    }
    builder
        .finish(std::fs::File::create(path).unwrap())
        .unwrap();
}
fn options(dir: &Path) -> Options {
    Options {
        input: dir.join("source.mdx"),
        output: dir.join("result.sqlite"),
        report: dir.join("report.json"),
        task_id: "test-task".into(),
        dictionary_id: Some("test-dictionary".into()),
        profile: "auto".into(),
    }
}
fn convert(dir: &Path, extra: bool) -> converter::Report {
    source(&dir.join("source.mdx"), extra);
    converter::convert(&options(dir), &AtomicBool::new(false), |_| {}).unwrap()
}

#[test]
fn real_mdx_to_sqlite_preserves_fields_and_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let report = convert(dir.path(), false);
    assert_eq!(report.entries, 4);
    assert_eq!(report.aliases, 1);
    assert_eq!(report.source_records, 5);
    assert_eq!(report.quality_status, "complete");
    let meta = database::validate(&options(dir.path()).output).unwrap();
    assert_eq!(meta.id, "test-dictionary");
    let conn = database::open_read(&options(dir.path()).output).unwrap();
    let hit = database::search(&conn, &meta.id, "LIGHT", "en").unwrap();
    assert_eq!(hit[0].word, "light");
    let item = database::detail(&conn, &meta.id, hit[0].entry_id).unwrap();
    assert_eq!(item.senses[0].pos, "n.");
    assert_eq!(item.senses[0].translation, "光；光线");
    assert_eq!(item.pronunciations[0].text, "laɪt");
    assert_eq!(item.examples[0].translation, "让光进来。");
    assert!(!item.body.contains("bad_script"));
    assert!(!item.body.contains("播放"));
    let alias = database::search(&conn, &meta.id, "shine", "auto").unwrap();
    assert_eq!(alias[0].word, "light");
    assert_eq!(alias[0].rank, 0);
    assert!(
        database::search(&conn, &meta.id, "银行", "en")
            .unwrap()
            .iter()
            .any(|c| c.word == "bank" && c.rank == 3)
    );
    assert!(
        database::search(&conn, &meta.id, "光", "en")
            .unwrap()
            .iter()
            .any(|c| c.word == "light")
    );
}
#[test]
fn homographs_from_one_dictionary_keep_all_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let mut builder = mdictlib::MdxBuilder::new();
    builder.header_attribute("Title", "同形词测试").unwrap();
    for text in ["wéi\nhello (when answering the phone)", "wèi\nto feed", "wèi\nto call out", "wéi\nhey"] {
        builder.add_entry("喂", text).unwrap();
    }
    builder.finish(std::fs::File::create(dir.path().join("source.mdx")).unwrap()).unwrap();
    let report = converter::convert(&options(dir.path()), &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.entries, 4);
    let mut store = Store::open(dir.path().join("app")).unwrap();
    store.import(&options(dir.path()).output, false).unwrap();
    let entries = store.sources("喂", "zh").unwrap();
    assert_eq!(entries.len(), 4);
    let bodies = entries.iter().map(|e| store.entry(&e.dictionary, e.entry_id).unwrap().body).collect::<Vec<_>>();
    for text in ["hello (when answering the phone)", "to feed", "to call out", "hey"] {
        assert!(bodies.iter().any(|b| b.contains(text)), "missing {text}");
    }
}
#[test]
fn quick_lookup_defaults_for_old_settings_and_keeps_disabled_state() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("dictlite.settings.json");
    std::fs::write(&config, r#"{"version":2,"general":{"suggestions":true,"history":true,"launchAtStartup":false,"closeToTray":true,"releaseMinutes":5},"dictionaries":[]}"#).unwrap();
    let mut store = Store::open(dir.path().to_path_buf()).unwrap();
    assert!(store.settings.general.quick_lookup);
    let mut settings = store.settings.clone();
    settings.general.quick_lookup = false;
    store.save_settings(settings).unwrap();
    drop(store);
    let reopened = Store::open(dir.path().to_path_buf()).unwrap();
    assert!(!reopened.settings.general.quick_lookup);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
    assert_eq!(value["general"]["quickLookup"], false);
}
#[test]
fn missing_text_and_alias_cycle_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let r = convert(dir.path(), true);
    assert_eq!(r.quality_status, "needs_review");
    assert_eq!(r.skipped, 2);
    assert_eq!(r.source_records, r.entries + r.aliases + r.skipped);
    let mut store = Store::open(dir.path().join("app")).unwrap();
    assert!(store.import(&options(dir.path()).output, false).is_err());
    assert!(store.import(&options(dir.path()).output, true).is_ok());
}
#[test]
fn invalid_and_empty_databases_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake.sqlite");
    std::fs::write(&path, b"SQLite format 3\0").unwrap();
    assert!(database::validate(&path).is_err());
    let plain = dir.path().join("plain.sqlite");
    rusqlite::Connection::open(&plain).unwrap();
    assert!(database::validate(&plain).is_err());
}
#[test]
fn cancellation_and_existing_output_preserve_originals() {
    let dir = tempfile::tempdir().unwrap();
    source(&dir.path().join("source.mdx"), false);
    let o = options(dir.path());
    assert_eq!(
        converter::convert(&o, &AtomicBool::new(true), |_| {}).unwrap_err(),
        "cancelled"
    );
    assert!(!o.output.exists());
    assert!(o.input.exists());
    std::fs::write(&o.output, b"keep").unwrap();
    assert!(converter::convert(&o, &AtomicBool::new(false), |_| {}).is_err());
    assert_eq!(std::fs::read(&o.output).unwrap(), b"keep");
}
#[test]
fn settings_collections_and_migration_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    convert(dir.path(), false);
    let root = dir.path().join("app");
    let mut store = Store::open(root.clone()).unwrap();
    store.import(&options(dir.path()).output, false).unwrap();
    assert!(store.import(&options(dir.path()).output, false).is_err());
    let mut settings = store.settings.clone();
    settings.dictionaries[0].name = "中文 <&> 词典".into();
    settings.dictionaries[0].examples = "expanded".into();
    settings.general.release_minutes = 0;
    store.save_settings(settings).unwrap();
    let record = Record {
        word: "light".into(),
        language: "en".into(),
        gloss: "光".into(),
        pronunciation: "laɪt".into(),
    };
    store.record_query(&record).unwrap();
    store.record_query(&record).unwrap();
    assert_eq!(store.records("history").unwrap().len(), 1);
    assert!(store.toggle_saved(&record).unwrap());
    let mut settings = store.settings.clone();
    settings.general.history = false;
    store.save_settings(settings).unwrap();
    store
        .record_query(&Record {
            word: "bank".into(),
            ..record.clone()
        })
        .unwrap();
    drop(store);
    let reopened = Store::open(root).unwrap();
    assert_eq!(reopened.settings.general.release_minutes, 0);
    assert_eq!(reopened.settings.dictionaries[0].name, "中文 <&> 词典");
    assert_eq!(reopened.records("saved").unwrap().len(), 1);
    assert_eq!(reopened.records("history").unwrap().len(), 1);
    assert_eq!(reopened.search("银行", "auto").unwrap()[0].word, "银行");
}
#[test]
fn malformed_settings_do_not_get_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dictlite.settings.json"), b"invalid").unwrap();
    assert!(Store::open(dir.path().to_path_buf()).is_err());
    assert_eq!(
        std::fs::read(dir.path().join("dictlite.settings.json")).unwrap(),
        b"invalid"
    );
}
#[test]
fn cleanup_removes_script_and_retains_paragraphs() {
    let (text, _) = converter::clean_html(
        "<style>hidden</style><p>first</p><p>second <b>word</b></p><iframe>hidden</iframe>",
    );
    assert_eq!(text, "first\nsecond word");
}
#[test]
fn concise_template_keeps_pronunciation_and_part_of_speech() {
    let (body, _) = converter::clean_html(
        "`1`light`2`\n`3`[lait] `4`-K3`2`\nn. 光, 光亮 </br>\na. 轻的 </br>\nvt. 点燃 </br>",
    );
    let (s, p) = converter::concise_fields(&body, "light");
    assert_eq!(s.len(), 3);
    assert_eq!(s[0].pos, "n.");
    assert_eq!(s[1].translation, "轻的");
    assert_eq!(p[0].text, "lait");
    assert!(!body.contains('`'));
}
#[test]
#[ignore]
fn emit_manual_fixture() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("DICTLITE_FIXTURE_DIR").expect("set DICTLITE_FIXTURE_DIR"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    convert(&dir, false);
    let mut store = Store::open(dir.join("app")).unwrap();
    store.import(&options(&dir).output, false).unwrap();
}

#[test]
#[ignore]
fn inspect_real_source() {
    let path = std::env::var_os("DICTLITE_SOURCE_PATH").unwrap();
    let source = mdictlib::MdxFile::open(path).unwrap();
    println!(
        "entries={} title={:?}",
        source.len(),
        source.header().title()
    );
    for word in ["light", "bank", "hello", "银行", "光"] {
        if let Some(e) = source.lookup(word).unwrap() {
            println!(
                "KEY={} BODY={}",
                e.key(),
                e.text().chars().take(2400).collect::<String>()
            );
        }
    }
}

#[test]
#[ignore]
fn validate_real_output_and_queries() {
    let path = std::path::PathBuf::from(
        std::env::var_os("DICTLITE_SQLITE_PATH").expect("set DICTLITE_SQLITE_PATH"),
    );
    let start = std::time::Instant::now();
    let meta = database::validate(&path).unwrap();
    println!(
        "VALIDATED entries={} quality={} elapsed_ms={}",
        meta.entries,
        meta.quality,
        start.elapsed().as_millis()
    );
    let conn = database::open_read(&path).unwrap();
    for (query, mode, expected) in [
        ("light", "en", "light"),
        ("bank", "en", "bank"),
        ("hello", "en", "hello"),
        ("银行", "auto", "bank"),
        ("光", "auto", "light"),
    ] {
        let start = std::time::Instant::now();
        let candidates = database::search(&conn, &meta.id, query, mode).unwrap();
        println!(
            "QUERY {query} elapsed_ms={} words={:?}",
            start.elapsed().as_millis(),
            candidates.iter().map(|c| &c.word).collect::<Vec<_>>()
        );
        assert!(
            candidates.iter().any(|c| c.word == expected),
            "missing expected {expected}"
        );
    }
    let light = database::search(&conn, &meta.id, "light", "en")
        .unwrap()
        .remove(0);
    let entry = database::detail(&conn, &meta.id, light.entry_id).unwrap();
    assert!(entry.senses.iter().any(|s| s.translation.contains('光')));
    assert!(!entry.pronunciations.is_empty());
    if let Some(root) = std::env::var_os("DICTLITE_APP_DIR") {
        let mut store = Store::open(root.into()).unwrap();
        if store.settings.dictionaries.is_empty() {
            store.import(&path, true).unwrap();
        }
        assert_eq!(store.snapshot().unwrap().libraries[0].entries, meta.entries);
    }
}
