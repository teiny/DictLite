#![windows_subsystem = "windows"]

#[cfg(windows)]
mod native_shortcut;

use dictlite::{
    Result, error,
    store::{Record, Settings, Store},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{
    Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

struct Job {
    task: String,
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
}
struct AppState {
    store: Mutex<Store>,
    job: Mutex<Option<Job>>,
    release: Arc<(Mutex<Option<Instant>>, Condvar)>,
    speech: AtomicU64,
    quitting: AtomicBool,
    session: Mutex<Value>,
    conversion_result: Mutex<Value>,
    popup: Mutex<Value>,
    popup_serial: AtomicU64,
    popup_pinned: AtomicBool,
    shortcut_error: Mutex<Option<String>>,
    #[cfg(windows)]
    shortcut: Mutex<Option<native_shortcut::Listener>>,
}
fn show(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    *state.release.0.lock().map_err(error)? = None;
    state.release.1.notify_one();
    if let Some(w) = app.get_webview_window("main") {
        w.unminimize().map_err(error)?;
        w.show().map_err(error)?;
        w.set_focus().map_err(error)?;
    } else {
        WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title("DictLite")
            .inner_size(1100.0, 820.0)
            .min_inner_size(320.0, 500.0)
            .data_directory(state.store.lock().map_err(error)?.root.join("data/webview"))
            .build()
            .map_err(error)?;
    }
    Ok(())
}
fn open_popup(app: &tauri::AppHandle, text: String, cursor: (i32, i32)) -> Result<()> {
    let state = app.state::<AppState>();
    if state.quitting.load(Ordering::SeqCst) || !state.store.lock().map_err(error)?.settings.general.quick_lookup { return Ok(()); }
    let existing = app.get_webview_window("popup");
    if existing.is_none() { state.popup_pinned.store(false, Ordering::SeqCst); }
    let revision = state.popup_serial.fetch_add(1, Ordering::SeqCst) + 1;
    let query = json!({"query":text,"revision":revision,"pinned":state.popup_pinned.load(Ordering::SeqCst)});
    *state.popup.lock().map_err(error)? = query.clone();
    let monitor = app.monitor_from_point(cursor.0 as f64, cursor.1 as f64).map_err(error)?
        .or(app.primary_monitor().map_err(error)?).ok_or("无法确定屏幕位置。")?;
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let width = (440.0 * scale).round().min(work.size.width as f64 - 8.0).max(100.0) as u32;
    let height = (380.0 * scale).round().min(work.size.height as f64 - 8.0).max(100.0) as u32;
    let position = dictlite::shortcut::popup_position(cursor,
        (work.position.x, work.position.y, work.position.x + work.size.width as i32, work.position.y + work.size.height as i32),
        (width as i32, height as i32));
    let window = if let Some(window) = existing { window } else {
        WebviewWindowBuilder::new(app, "popup", WebviewUrl::App("popup.html".into()))
            .title("DictLite 快速查词").inner_size(440.0, 380.0)
            .decorations(false).resizable(false).skip_taskbar(true)
            .visible(false).focused(false)
            .data_directory(state.store.lock().map_err(error)?.root.join("data/webview"))
            .build().map_err(error)?
    };
    window.set_size(tauri::PhysicalSize::new(width, height)).map_err(error)?;
    window.set_position(tauri::PhysicalPosition::new(position.0, position.1)).map_err(error)?;
    window.show().map_err(error)?;
    window.set_focus().map_err(error)?;
    window.emit("popup-query", query).map_err(error)?;
    Ok(())
}
fn stop_shortcut(app: &tauri::AppHandle) {
    #[cfg(windows)]
    if let Some(mut listener) = app.state::<AppState>().shortcut.lock().unwrap().take() { listener.stop(); }
}
fn start_shortcut(app: &tauri::AppHandle) -> Result<()> {
    #[cfg(windows)]
    {
        let state = app.state::<AppState>();
        let mut listener = state.shortcut.lock().map_err(error)?;
        if listener.is_none() {
            match native_shortcut::Listener::start(app.clone()) {
                Ok(value) => { *listener = Some(value); *state.shortcut_error.lock().map_err(error)? = None; }
                Err(e) => { *state.shortcut_error.lock().map_err(error)? = Some(e.clone()); return Err(format!("无法启用快捷查词：{e}")); }
            }
        }
    }
    Ok(())
}
fn cancel(app: &tauri::AppHandle) -> Result<()> {
    if let Some(job) = app.state::<AppState>().job.lock().map_err(error)?.as_ref() {
        let message = json!({"protocol_version":1,"task_id":job.task,"command":"cancel"});
        writeln!(job.stdin.lock().map_err(error)?, "{message}").map_err(error)?;
    }
    Ok(())
}
fn quit(app: &tauri::AppHandle) {
    if app
        .state::<AppState>()
        .quitting
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    app.state::<AppState>()
        .speech
        .fetch_add(1, Ordering::SeqCst);
    app.state::<AppState>().release.1.notify_one();
    stop_shortcut(app);
    let _ = cancel(app);
    let app = app.clone();
    std::thread::spawn(move || {
        for _ in 0..50 {
            if app.state::<AppState>().job.lock().unwrap().is_none() {
                cleanup_ready(app.clone());
                app.exit(0);
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(job) = app.state::<AppState>().job.lock().unwrap().as_ref() {
            let _ = job.child.lock().unwrap().kill();
            let _ = job.child.lock().unwrap().wait();
            let path = app
                .state::<AppState>()
                .store
                .lock()
                .unwrap()
                .root
                .join("work")
                .join(&job.task);
            let _ = fs::remove_dir_all(path);
        }
        app.exit(0);
    });
}
fn cleanup_ready(app: tauri::AppHandle) {
    let state = app.state::<AppState>();
    let value = state.conversion_result.lock().unwrap().clone();
    if let Some(task) = value["task_id"].as_str() {
        if uuid::Uuid::parse_str(task).is_ok() {
            let path = state.store.lock().unwrap().root.join("work").join(task);
            let _ = fs::remove_dir_all(path);
        }
    }
}
fn autostart(enabled: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
            .map_err(error)?;
        if enabled {
            key.set_value(
                "DictLite",
                &format!(
                    "\"{}\" --background",
                    std::env::current_exe().map_err(error)?.display()
                ),
            )
            .map_err(error)?;
        } else if let Err(e) = key.delete_value("DictLite") {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(error(e));
            }
        }
    }
    #[cfg(not(windows))]
    if enabled {
        return Err("当前系统暂未接入随系统启动。".into());
    }
    Ok(())
}
fn save(app: &tauri::AppHandle, settings: Settings) -> Result<()> {
    settings.validate()?;
    let state = app.state::<AppState>();
    let mut store = state.store.lock().map_err(error)?;
    let old = store.settings.general.launch_at_startup;
    let next = settings.general.launch_at_startup;
    let enable_shortcut = !store.settings.general.quick_lookup && settings.general.quick_lookup;
    let disable_shortcut = store.settings.general.quick_lookup && !settings.general.quick_lookup;
    if enable_shortcut { start_shortcut(app)?; }
    if old != next {
        if let Err(e) = autostart(next) {
            if enable_shortcut { stop_shortcut(app); }
            return Err(e);
        }
    }
    if let Err(e) = store.save_settings(settings) {
        if old != next {
            let _ = autostart(old);
        }
        if enable_shortcut { stop_shortcut(app); }
        return Err(e);
    }
    if disable_shortcut {
        stop_shortcut(app);
        *state.shortcut_error.lock().map_err(error)? = None;
        if let Some(window) = app.get_webview_window("popup") { let _ = window.destroy(); }
    }
    Ok(())
}
fn start_conversion(app: &tauri::AppHandle, input: &str) -> Result<Value> {
    let state = app.state::<AppState>();
    let mut active = state.job.lock().map_err(error)?;
    if active.is_some() {
        return Err("已有转换任务，请等待完成或取消。".into());
    }
    *state.conversion_result.lock().map_err(error)? = Value::Null;
    let root = state.store.lock().map_err(error)?.root.clone();
    let task = uuid::Uuid::new_v4().to_string();
    let work = root.join("work").join(&task);
    fs::create_dir_all(&work).map_err(error)?;
    let exe = std::env::current_exe()
        .map_err(error)?
        .with_file_name("dictlite-convert.exe");
    if !exe.exists() {
        let _ = fs::remove_dir(&work);
        return Err("缺少 dictlite-convert 转换程序，请使用完整程序目录。".into());
    }
    let mut command = Command::new(exe);
    command
        .args(["convert", "--input", input, "--output"])
        .arg(work.join("dictionary.sqlite"))
        .arg("--report")
        .arg(work.join("report.json"))
        .args(["--task-id", &task])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(error)?;
    let stdout = child.stdout.take().ok_or("转换程序输出不可用")?;
    let stderr = child.stderr.take().ok_or("转换程序错误输出不可用")?;
    let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("转换程序输入不可用")?));
    let child = Arc::new(Mutex::new(child));
    *active = Some(Job {
        task: task.clone(),
        child: child.clone(),
        stdin,
    });
    let app = app.clone();
    let job_id = task.clone();
    std::thread::spawn(move || {
        let errors = std::thread::spawn(move || {
            BufReader::new(stderr)
                .lines()
                .map_while(std::result::Result::ok)
                .take(100)
                .collect::<Vec<_>>()
                .join("\n")
        });
        let mut final_event = None;
        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v["task_id"] == job_id && v["protocol_version"] == 1 {
                    if v["type"] == "finished" {
                        final_event = Some(v.clone());
                    } else {
                        let _ = app.emit("conversion", v);
                    }
                }
            }
        }
        let status = child.lock().unwrap().wait();
        let stderr = errors.join().unwrap_or_default();
        let mut result=final_event.unwrap_or_else(||json!({"type":"finished","task_id":job_id,"status":"failed","error":"转换程序意外退出。"}));
        if !status.as_ref().is_ok_and(|s| s.success()) && result["status"] == "succeeded" {
            result["status"] = json!("failed");
            result["error"] = json!(stderr);
        }
        if result["status"] == "succeeded" {
            if let Err(e) = dictlite::database::validate(&work.join("dictionary.sqlite")) {
                result["status"] = json!("failed");
                result["error"] = json!(e);
            } else {
                let report = fs::read(work.join("report.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                if let Some(report) = report {
                    result["details"] = report;
                } else {
                    result["status"] = json!("failed");
                    result["error"] = json!("转换报告缺失或无效。");
                }
            }
        }
        if result["status"] != "succeeded" {
            let _ = fs::remove_dir_all(&work);
        }
        *app.state::<AppState>().conversion_result.lock().unwrap() = result.clone();
        *app.state::<AppState>().job.lock().unwrap() = None;
        let _ = app.emit("conversion", result);
    });
    Ok(json!({"task_id":task}))
}

fn execute(app: &tauri::AppHandle, action: &str, payload: Value) -> Result<Value> {
    let text = |key: &str| {
        payload[key]
            .as_str()
            .ok_or_else(|| format!("缺少参数 {key}"))
    };
    match action {
        "snapshot" => {
            let mut snapshot = serde_json::to_value(
            app.state::<AppState>()
                .store
                .lock()
                .map_err(error)?
                .snapshot()?,
        )
        .map_err(error)?;
            snapshot["shortcut_error"] = json!(*app.state::<AppState>().shortcut_error.lock().map_err(error)?);
            Ok(snapshot)
        },
        "search" => serde_json::to_value(
            app.state::<AppState>()
                .store
                .lock()
                .map_err(error)?
                .search(text("query")?, text("mode")?)?,
        )
        .map_err(error),
        "lookup" => serde_json::to_value(
            app.state::<AppState>()
                .store
                .lock()
                .map_err(error)?
                .sources(text("word")?, text("language")?)?,
        )
        .map_err(error),
        "detail" => {
            let state = app.state::<AppState>();
            let store = state.store.lock().map_err(error)?;
            let item = store.entry(
                text("dictionary")?,
                payload["entry_id"].as_i64().ok_or("词条标识无效")?,
            )?;
            if payload["record"].as_bool().unwrap_or(false) {
                store.record_query(&Record {
                    word: item.word.clone(),
                    language: item.language.clone(),
                    gloss: item.gloss.clone(),
                    pronunciation: item
                        .pronunciations
                        .first()
                        .map(|p| p.text.clone())
                        .unwrap_or_default(),
                })?;
            }
            serde_json::to_value(item).map_err(error)
        }
        "save_settings" => {
            let value: Settings = serde_json::from_value(payload).map_err(error)?;
            save(app, value)?;
            Ok(json!(true))
        }
        "toggle_saved" => {
            let record: Record = serde_json::from_value(payload).map_err(error)?;
            Ok(json!(
                app.state::<AppState>()
                    .store
                    .lock()
                    .map_err(error)?
                    .toggle_saved(&record)?
            ))
        }
        "clear_history" => {
            app.state::<AppState>()
                .store
                .lock()
                .map_err(error)?
                .clear_history()?;
            Ok(json!(true))
        }
        "choose_dictionary" => {
            let format = text("format")?;
            let dialog = rfd::FileDialog::new().set_title("选择词典");
            let dialog = if format == "mdx" {
                dialog.add_filter("MDX 词典", &["mdx"])
            } else {
                dialog.add_filter("DictLite 词典", &["sqlite", "sqlite3", "db"])
            };
            Ok(json!(dialog.pick_file().map(|p| p.display().to_string())))
        }
        "inspect_dictionary" => Ok(json!(dictlite::database::metadata(Path::new(text(
            "path"
        )?))?)),
        "import_dictionary" => {
            let meta = app
                .state::<AppState>()
                .store
                .lock()
                .map_err(error)?
                .import(
                    Path::new(text("path")?),
                    payload["accept_review"].as_bool().unwrap_or(false),
                )?;
            Ok(json!(meta))
        }
        "convert" => start_conversion(app, text("path")?),
        "cancel_conversion" => {
            cancel(app)?;
            Ok(json!(true))
        }
        "conversion_status" => {
            let state = app.state::<AppState>();
            let task = state
                .job
                .lock()
                .map_err(error)?
                .as_ref()
                .map(|j| j.task.clone());
            Ok(
                json!({"active":task,"result":state.conversion_result.lock().map_err(error)?.clone()}),
            )
        }
        "read_report" => {
            let path = PathBuf::from(text("path")?);
            let root = app
                .state::<AppState>()
                .store
                .lock()
                .map_err(error)?
                .root
                .join("work");
            let real = fs::canonicalize(&path).map_err(error)?;
            if !real.starts_with(fs::canonicalize(root).map_err(error)?) {
                return Err("报告路径无效。".into());
            }
            serde_json::from_slice(&fs::read(real).map_err(error)?).map_err(error)
        }
        "cleanup_conversion" => {
            let task = text("task_id")?;
            uuid::Uuid::parse_str(task).map_err(error)?;
            if app
                .state::<AppState>()
                .job
                .lock()
                .map_err(error)?
                .as_ref()
                .is_some_and(|j| j.task == task)
            {
                return Err("任务仍在运行。".into());
            }
            let path = app
                .state::<AppState>()
                .store
                .lock()
                .map_err(error)?
                .root
                .join("work")
                .join(task);
            if path.exists() {
                fs::remove_dir_all(path).map_err(error)?;
            }
            *app.state::<AppState>()
                .conversion_result
                .lock()
                .map_err(error)? = Value::Null;
            Ok(json!(true))
        }
        "export_settings" => {
            let value: Settings = serde_json::from_value(payload).map_err(error)?;
            value.validate()?;
            if let Some(path) = rfd::FileDialog::new()
                .set_title("导出设置")
                .set_file_name("dictlite.settings.json")
                .add_filter("JSON 设置", &["json"])
                .save_file()
            {
                fs::write(path, serde_json::to_vec_pretty(&value).map_err(error)?)
                    .map_err(error)?;
                Ok(json!(true))
            } else {
                Ok(json!(false))
            }
        }
        "import_settings" => {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("导入设置")
                .add_filter("JSON 设置", &["json"])
                .pick_file()
            {
                let value: Settings =
                    serde_json::from_slice(&fs::read(path).map_err(error)?).map_err(error)?;
                save(app, value)?;
                Ok(json!(true))
            } else {
                Ok(json!(false))
            }
        }
        "session" => {
            *app.state::<AppState>().session.lock().map_err(error)? = payload;
            Ok(json!(true))
        }
        "get_session" => Ok(app
            .state::<AppState>()
            .session
            .lock()
            .map_err(error)?
            .clone()),
        "popup_query" => {
            let state = app.state::<AppState>();
            let mut query = state.popup.lock().map_err(error)?.clone();
            if query.is_object() { query["pinned"] = json!(state.popup_pinned.load(Ordering::SeqCst)); }
            Ok(query)
        }
        "popup_ready" => {
            let state = app.state::<AppState>();
            if payload["revision"].as_u64() == Some(state.popup_serial.load(Ordering::SeqCst)) {
                if let Some(window) = app.get_webview_window("popup") { window.set_focus().map_err(error)?; }
            }
            Ok(json!(true))
        }
        "set_popup_pinned" => {
            let pinned = payload["pinned"].as_bool().ok_or("置顶状态无效")?;
            let state = app.state::<AppState>();
            if let Some(window) = app.get_webview_window("popup") {
                let old = state.popup_pinned.swap(pinned, Ordering::SeqCst);
                if let Err(e) = window.set_always_on_top(pinned) { state.popup_pinned.store(old, Ordering::SeqCst); return Err(error(e)); }
                if !pinned && !window.is_focused().map_err(error)? { window.destroy().map_err(error)?; }
            }
            Ok(json!(pinned))
        }
        "close_popup" => {
            if let Some(window) = app.get_webview_window("popup") { window.destroy().map_err(error)?; }
            Ok(json!(true))
        }
        "open_main_entry" => {
            let session = json!({"view":"search","word":text("word")?,"language":text("language")?,"source":payload["source"]});
            *app.state::<AppState>().session.lock().map_err(error)? = session.clone();
            show(app)?;
            app.emit_to("main", "open-entry", session).map_err(error)?;
            if let Some(window) = app.get_webview_window("popup") { window.destroy().map_err(error)?; }
            Ok(json!(true))
        }
        "speak" => {
            speak(app, text("text")?, text("language")?)?;
            Ok(json!(true))
        }
        "quit" => {
            quit(app);
            Ok(json!(true))
        }
        _ => Err("未知操作。".into()),
    }
}
#[tauri::command]
async fn dispatch(app: tauri::AppHandle, action: String, payload: Value) -> Result<Value> {
    tauri::async_runtime::spawn_blocking(move || execute(&app, &action, payload))
        .await
        .map_err(error)?
}

fn speak(app: &tauri::AppHandle, text: &str, lang: &str) -> Result<()> {
    if text.chars().count() > 256 {
        return Err("朗读内容过长。".into());
    }
    #[cfg(windows)]
    unsafe {
        use windows::{
            Win32::{
                Media::Speech::{
                    ISpeechVoice, SVSFIsNotXML, SVSFPurgeBeforeSpeak, SVSFlagsAsync, SpVoice,
                    SpeechVoiceSpeakFlags,
                },
                System::Com::{
                    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                    CoUninitialize,
                },
            },
            core::BSTR,
        };
        let generation = app
            .state::<AppState>()
            .speech
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(error)?;
        let result = (|| {
            let voice: ISpeechVoice =
                CoCreateInstance(&SpVoice, None, CLSCTX_ALL).map_err(error)?;
            let locale = match lang {
                "en-GB" => "809",
                "en-US" => "409",
                _ => "804",
            };
            let voices = voice
                .GetVoices(&BSTR::from(format!("Language={locale}")), &BSTR::new())
                .map_err(error)?;
            if voices.Count().map_err(error)? == 0 {
                return Err("系统没有对应的本地语音。".into());
            }
            voice
                .putref_Voice(&voices.Item(0).map_err(error)?)
                .map_err(error)?;
            voice.SetRate(-1).map_err(error)?;
            voice
                .Speak(
                    &BSTR::from(text),
                    SpeechVoiceSpeakFlags(
                        SVSFlagsAsync.0 | SVSFPurgeBeforeSpeak.0 | SVSFIsNotXML.0,
                    ),
                )
                .map_err(error)?;
            while !voice.WaitUntilDone(100).map_err(error)?.as_bool() {
                if app.state::<AppState>().speech.load(Ordering::SeqCst) != generation {
                    voice
                        .Speak(&BSTR::new(), SVSFPurgeBeforeSpeak)
                        .map_err(error)?;
                    break;
                }
            }
            Ok(())
        })();
        CoUninitialize();
        result
    }
    #[cfg(not(windows))]
    {
        let _ = (text, lang);
        Err("当前系统尚未接入 TTS。".into())
    }
}
fn main() {
    let root = std::env::var_os("DICTLITE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf()
        });
    let store = match Store::open(root) {
        Ok(s) => s,
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("DictLite 无法启动")
                .set_description(&e)
                .set_level(rfd::MessageLevel::Error)
                .show();
            return;
        }
    };
    let mut builder = tauri::Builder::default();
    // Explicit data directories are isolated profiles, also used by integration checks.
    if std::env::var_os("DICTLITE_DATA_DIR").is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = show(app);
        }));
    }
    let app = builder
        .manage(AppState {
            store: Mutex::new(store),
            job: Mutex::new(None),
            release: Arc::new((Mutex::new(None), Condvar::new())),
            speech: AtomicU64::new(0),
            quitting: AtomicBool::new(false),
            session: Mutex::new(Value::Null),
            conversion_result: Mutex::new(Value::Null),
            popup: Mutex::new(Value::Null),
            popup_serial: AtomicU64::new(0),
            popup_pinned: AtomicBool::new(false),
            shortcut_error: Mutex::new(None),
            #[cfg(windows)]
            shortcut: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![dispatch])
        .setup(|app| {
            if app.state::<AppState>().store.lock().unwrap().settings.general.quick_lookup {
                let _ = start_shortcut(app.handle());
            }
            let timer_app = app.handle().clone();
            let release = app.state::<AppState>().release.clone();
            std::thread::spawn(move || {
                let mut deadline = release.0.lock().unwrap();
                loop {
                    if timer_app
                        .state::<AppState>()
                        .quitting
                        .load(Ordering::SeqCst)
                    {
                        break;
                    }
                    if let Some(end) = *deadline {
                        let remaining = end.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            *deadline = None;
                            if let Some(w) = timer_app.get_webview_window("main") {
                                if !w.is_visible().unwrap_or(true) {
                                    timer_app
                                        .state::<AppState>()
                                        .speech
                                        .fetch_add(1, Ordering::SeqCst);
                                    let _ = w.destroy();
                                }
                            }
                        } else {
                            deadline = release.1.wait_timeout(deadline, remaining).unwrap().0;
                        }
                    } else {
                        deadline = release.1.wait(deadline).unwrap();
                    }
                }
            });
            let show_item = MenuItem::with_id(app, "show", "打开 DictLite", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/icon.png"
                ))?)
                .tooltip("DictLite")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "show" => {
                        let _ = show(app);
                    }
                    "quit" => quit(app),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if matches!(
                        e,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        let _ = show(tray.app_handle());
                    }
                })
                .build(app)?;
            if !std::env::args().any(|a| a == "--background") {
                show(app.handle()).map_err(std::io::Error::other)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "popup" {
                match event {
                    WindowEvent::CloseRequested { api, .. } => { api.prevent_close(); let _ = window.destroy(); }
                    WindowEvent::Focused(false) if window.is_visible().unwrap_or(false) && !window.app_handle().state::<AppState>().popup_pinned.load(Ordering::SeqCst) => { let _ = window.destroy(); }
                    _ => {}
                }
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let state = app.state::<AppState>();
                let general = state.store.lock().unwrap().settings.general.clone();
                api.prevent_close();
                if !general.close_to_tray {
                    quit(app);
                    return;
                }
                let _ = window.hide();
                *state.release.0.lock().unwrap() = Some(
                    Instant::now() + Duration::from_secs(u64::from(general.release_minutes) * 60),
                );
                state.release.1.notify_one();
            }
        })
        .build(tauri::generate_context!())
        .expect("无法创建 DictLite 应用");
    app.run(|app, event| {
        if let RunEvent::Exit = event { stop_shortcut(app); }
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if code.is_none() && !app.state::<AppState>().quitting.load(Ordering::SeqCst) {
                api.prevent_exit();
            }
        }
    });
}
