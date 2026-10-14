use dictlite::converter::{self, Options};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|s| s == "--help") {
        println!(
            "dictlite-convert convert --input source.mdx --output result.sqlite --report report.json --task-id task-001 [--profile auto|generic|concise] [--dictionary-id ID]"
        );
        return;
    }
    if args[0] != "convert" || args[1..].len() % 2 != 0 {
        eprintln!("参数无效，请使用 --help 查看用法。");
        std::process::exit(2);
    }
    let mut map = std::collections::HashMap::new();
    for pair in args[1..].chunks(2) {
        if ![
            "--input",
            "--output",
            "--report",
            "--task-id",
            "--profile",
            "--dictionary-id",
        ]
        .contains(&pair[0].as_str())
            || map.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            eprintln!("未知或重复参数：{}", pair[0]);
            std::process::exit(2);
        }
    }
    for key in ["--input", "--output", "--report", "--task-id"] {
        if !map.contains_key(key) {
            eprintln!("缺少参数 {key}");
            std::process::exit(2);
        }
    }
    let options = Options {
        input: PathBuf::from(&map["--input"]),
        output: PathBuf::from(&map["--output"]),
        report: PathBuf::from(&map["--report"]),
        task_id: map["--task-id"].clone(),
        profile: map.get("--profile").cloned().unwrap_or("auto".into()),
        dictionary_id: map.get("--dictionary-id").cloned(),
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let task = options.task_id.clone();
    std::thread::spawn(move || {
        for line in io::stdin()
            .lock()
            .lines()
            .map_while(std::result::Result::ok)
        {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v["command"] == "cancel" && v["task_id"] == task && v["protocol_version"] == 1 {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    let mut seq = 0;
    let mut emit = |mut v: Value| {
        seq += 1;
        v["protocol_version"] = json!(1);
        v["task_id"] = json!(options.task_id);
        v["seq"] = json!(seq);
        println!("{v}");
        let _ = io::stdout().flush();
    };
    if let Err(e) = converter::convert(&options, &cancelled, &mut emit) {
        let cancel = e == "cancelled";
        emit(json!({"type":"finished","status":if cancel{"cancelled"}else{"failed"},"error":e}));
        eprintln!("{e}");
        std::process::exit(if cancel { 6 } else { 4 });
    }
}
