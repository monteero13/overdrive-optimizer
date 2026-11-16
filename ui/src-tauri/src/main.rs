//! Overdrive desktop shell (ipc-protocol.md §5).
//!
//! - A **relay**: one connection to `\\.\pipe\overdrive.v1`, frames forwarded both ways.
//!   The WebView can only call `ipc_send` / `ipc_status` and listen to two events.
//! - The window is destroyed when closed (WebView2 memory goes back to the system); the
//!   process stays in the tray with the pipe connection alive. "Salir" quits for real.
//! - In release builds the relay only talks to a pipe served from session 0, i.e. by a
//!   Windows service, so a user-level process squatting the name gets nothing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

#[derive(Default)]
struct Relay {
    tx: Mutex<Option<Sender<String>>>,
    up: AtomicBool,
}

/// Request ids the shell itself uses (tray actions); far above anything the UI reaches.
static OWN_ID: AtomicU64 = AtomicU64::new(1 << 50);

fn pipe_name() -> String {
    if cfg!(debug_assertions) {
        if let Ok(p) = std::env::var("OVERDRIVE_PIPE") {
            return p;
        }
    }
    overdrive_ipc::PIPE_NAME.to_string()
}

fn trusted(p: &overdrive_ipc::Pipe) -> bool {
    if cfg!(debug_assertions) {
        return true; // dev: `overdrive-svc console` runs in the user's session
    }
    p.peer_pid()
        .ok()
        .and_then(overdrive_ipc::session_of)
        .is_some_and(|s| s == 0)
}

fn set_up(app: &AppHandle, relay: &Relay, up: bool) {
    relay.up.store(up, Ordering::SeqCst);
    let _ = app.emit("ipc://status", up);
}

fn relay_loop(app: AppHandle, relay: Arc<Relay>) {
    loop {
        let pipe = match overdrive_ipc::connect(&pipe_name(), Duration::from_secs(2)) {
            Ok(p) if trusted(&p) => p,
            Ok(p) => {
                p.close();
                thread::sleep(Duration::from_secs(2));
                continue;
            }
            Err(_) => {
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let (tx, rx) = mpsc::channel::<String>();
        *relay.tx.lock().unwrap() = Some(tx);
        set_up(&app, &relay, true);
        let w = pipe.clone();
        let writer = thread::spawn(move || {
            for f in rx {
                if w.write_frame(&f).is_err() {
                    w.close();
                    break;
                }
            }
        });
        while let Ok(f) = pipe.read_frame() {
            let _ = app.emit("ipc://frame", f);
        }
        pipe.close();
        *relay.tx.lock().unwrap() = None; // ends the writer
        let _ = writer.join();
        set_up(&app, &relay, false);
        thread::sleep(Duration::from_millis(500));
    }
}

#[tauri::command]
fn ipc_send(frame: String, relay: State<'_, Arc<Relay>>) -> Result<(), String> {
    if frame.len() > overdrive_ipc::MAX_FRAME {
        return Err("frame too large".into());
    }
    match relay.tx.lock().unwrap().as_ref() {
        Some(tx) => tx.send(frame).map_err(|_| "disconnected".into()),
        None => Err("disconnected".into()),
    }
}

#[tauri::command]
fn ipc_status(relay: State<'_, Arc<Relay>>) -> bool {
    relay.up.load(Ordering::SeqCst)
}

/// Public repository of community measurements (issues) and presets.
const REPO: &str = "monteero13/overdrive-optimizer";
const PRESETS_URL: &str =
    "https://raw.githubusercontent.com/monteero13/overdrive-optimizer/master/presets/community.json";
const MAX_PRESETS: u64 = 2 * 1024 * 1024;

/// A "new issue" URL of our repository, percent-encoded by the service: nothing else
/// can be opened (no arbitrary URLs, no files, no programs).
fn is_issue_url(url: &str) -> bool {
    url.strip_prefix(&format!("https://github.com/{REPO}/issues/new?"))
        .is_some_and(|q| q.bytes().all(|b| b.is_ascii_alphanumeric() || b"%&=-_.~".contains(&b)))
        && url.len() < 60_000
}

/// The user pressed "Open in GitHub" after reading the exact payload: open the default
/// browser on the pre-filled issue. Sending is still the user's click in GitHub.
#[tauri::command]
fn open_issue(url: String) -> Result<(), String> {
    if !is_issue_url(&url) {
        return Err("not an Overdrive issue URL".into());
    }
    std::process::Command::new("explorer.exe")
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Download the public community presets file (only called when the user opted in).
/// The service itself never touches the network.
#[tauri::command]
async fn fetch_presets() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut res = ureq::get(PRESETS_URL)
            .config()
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .call()
            .map_err(|e| e.to_string())?;
        res.body_mut()
            .with_config()
            .limit(MAX_PRESETS)
            .read_to_string()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn send_own(app: &AppHandle, method: &str) {
    let relay = app.state::<Arc<Relay>>();
    let id = OWN_ID.fetch_add(1, Ordering::Relaxed);
    let f = serde_json::json!({ "kind": "req", "id": id, "method": method, "params": {} }).to_string();
    let guard = relay.tx.lock().unwrap();
    if let Some(tx) = guard.as_ref() {
        let _ = tx.send(f);
    }
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("Overdrive")
        .inner_size(1280.0, 820.0)
        .min_inner_size(960.0, 640.0)
        .center()
        .build();
}

fn main() {
    let tray_only = std::env::args().any(|a| a == "--tray");
    let relay = Arc::new(Relay::default());
    let app = tauri::Builder::default()
        .manage(relay.clone())
        .invoke_handler(tauri::generate_handler![ipc_send, ipc_status, open_issue, fetch_presets])
        .setup(move |app| {
            let h = app.handle().clone();
            thread::spawn(move || relay_loop(h, relay));
            let open = MenuItem::with_id(app, "open", "Abrir Overdrive", true, None::<&str>)?;
            let resume = MenuItem::with_id(app, "resume", "Reanudar apps en pausa", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&open, &resume, &sep, &quit])?;
            let mut tray = TrayIconBuilder::with_id("overdrive")
                .tooltip("Overdrive")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_window(app),
                    "resume" => send_own(app, "background.resumeAll"),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|t, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        show_window(t.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            if !tray_only {
                show_window(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri app");
    app.run(|_, e| {
        // Closing the last window keeps the tray alive; only "Salir" (exit code set) quits.
        if let RunEvent::ExitRequested { api, code: None, .. } = e {
            api.prevent_exit();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::is_issue_url;

    #[test]
    fn only_prefilled_issues_of_our_repo_can_be_opened() {
        assert!(is_issue_url("https://github.com/monteero13/overdrive-optimizer/issues/new?title=%5Bx%5D&body=a%20b"));
        assert!(!is_issue_url("https://github.com/someone/else/issues/new?title=x"));
        assert!(!is_issue_url("https://evil.example/?https://github.com/monteero13/overdrive-optimizer/issues/new?"));
        assert!(!is_issue_url("https://github.com/monteero13/overdrive-optimizer/issues/new?title=x\" & calc"));
        assert!(!is_issue_url("file:///C:/Windows/System32/calc.exe"));
    }
}
