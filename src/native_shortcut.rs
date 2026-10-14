use dictlite::{Result, error, shortcut::CopyGesture};
use std::{
    cell::RefCell,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HGLOBAL, HINSTANCE, LPARAM, LRESULT, POINT, WPARAM},
        System::{
            DataExchange::{
                CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Threading::GetCurrentThreadId,
        },
        UI::WindowsAndMessaging::*,
    },
    core::PCWSTR,
};

struct HookState {
    gesture: CopyGesture,
    started: Instant,
    baseline: u32,
    sender: mpsc::SyncSender<u32>,
}
thread_local! { static HOOK: RefCell<Option<HookState>> = const { RefCell::new(None) }; }

unsafe extern "system" fn keyboard(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 {
        let down = message.0 == WM_KEYDOWN as usize || message.0 == WM_SYSKEYDOWN as usize;
        let up = message.0 == WM_KEYUP as usize || message.0 == WM_SYSKEYUP as usize;
        if down || up {
            let key = unsafe { &*(data.0 as *const KBDLLHOOKSTRUCT) }.vkCode;
            HOOK.with(|slot| {
                if let Some(state) = slot.borrow_mut().as_mut() {
                    let now = state.started.elapsed().as_millis() as u64;
                    if state.gesture.starts_copy(key, down, now) {
                        state.baseline = unsafe { GetClipboardSequenceNumber() };
                    }
                    if state.gesture.key(key, down, now) {
                        let _ = state.sender.try_send(state.baseline);
                    }
                }
            });
        }
    }
    unsafe { CallNextHookEx(None, code, message, data) }
}

fn clipboard_text(baseline: u32) -> Option<String> {
    // Copy is handled by the foreground app after our hook returns.
    for _ in 0..8 {
        unsafe {
            if GetClipboardSequenceNumber() != baseline && OpenClipboard(None).is_ok() {
                let text = (|| {
                    let handle = HGLOBAL(GetClipboardData(13).ok()?.0);
                    let units = (GlobalSize(handle) / 2).min(257);
                    let pointer = GlobalLock(handle) as *const u16;
                    if pointer.is_null() {
                        return None;
                    }
                    let data = std::slice::from_raw_parts(pointer, units);
                    let text = data
                        .iter()
                        .position(|&c| c == 0)
                        .and_then(|end| String::from_utf16(&data[..end]).ok());
                    let _ = GlobalUnlock(handle);
                    text
                })();
                let _ = CloseClipboard();
                return text
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty() && s.chars().count() <= 256);
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    None
}

pub struct Listener {
    thread_id: u32,
    hook: Option<thread::JoinHandle<()>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Listener {
    pub fn start(app: tauri::AppHandle) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (ready, status) = mpsc::sync_channel(1);
        let hook = thread::spawn(move || unsafe {
            let mut message = MSG::default();
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            let result = GetModuleHandleW(PCWSTR::null()).and_then(|module| {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), Some(HINSTANCE(module.0)), 0)
            });
            match result {
                Ok(handle) => {
                    HOOK.with(|slot| {
                        *slot.borrow_mut() = Some(HookState {
                            gesture: CopyGesture::default(),
                            started: Instant::now(),
                            baseline: 0,
                            sender,
                        })
                    });
                    let _ = ready.send(Ok(GetCurrentThreadId()));
                    while GetMessageW(&mut message, None, 0, 0).0 > 0 {}
                    let _ = UnhookWindowsHookEx(handle);
                    HOOK.with(|slot| *slot.borrow_mut() = None);
                }
                Err(e) => {
                    let _ = ready.send(Err(error(e)));
                }
            }
        });
        let thread_id = status.recv().map_err(error)??;
        let worker = thread::spawn(move || {
            while let Ok(baseline) = receiver.recv() {
                thread::sleep(Duration::from_millis(120));
                if let Some(text) = clipboard_text(baseline) {
                    let mut cursor = POINT::default();
                    if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
                        let popup_app = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            if let Err(e) =
                                crate::open_popup(&popup_app, text, (cursor.x, cursor.y))
                            {
                                let _ = popup_app.emit("shortcut-error", e);
                            }
                        });
                    }
                }
            }
        });
        Ok(Self {
            thread_id,
            hook: Some(hook),
            worker: Some(worker),
        })
    }
    pub fn stop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.hook.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.worker.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if self.hook.is_some() {
            self.stop();
        }
    }
}

use tauri::Emitter;
