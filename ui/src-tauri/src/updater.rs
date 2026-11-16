//! Updates (goals.md §8): signed manifests, staged rollout, automatic rollback.
//!
//! - **Signed:** `latest.json` and every installer are verified with the minisign public
//!   key in tauri.conf.json (`plugins.updater.pubkey`) before anything runs.
//! - **Staged rollout:** the manifest carries `"rollout": 0..=100`. Each install has a
//!   fixed random bucket 0–99 and only sees the update when `bucket < rollout`. Setting
//!   `rollout` to 0 halts a release for everyone who hasn't installed it yet.
//! - **Rollback:** before installing, we remember the version we came from. On the first
//!   start of the new version, if the service doesn't answer within [`HEALTH_WAIT`], the
//!   previous release's signed installer is downloaded and installed back (once).
//! - **The user decides** when to install (it needs administrator rights): the UI shows a
//!   prompt, nothing installs on its own except the rollback of a broken update.
//!
//! The WebView only gets `update_status` and `update_install`; the plugin's own commands
//! are not granted.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::Relay;

pub const REPO: &str = "monteero13/overdrive-optimizer";
const HEALTH_WAIT: Duration = Duration::from_secs(120);
const CHECK_EVERY: Duration = Duration::from_secs(12 * 3600);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Persisted {
    /// Fixed per install: which slice of a staged rollout this PC belongs to.
    bucket: Option<u8>,
    /// Set right before installing an update; cleared once the new version proved healthy.
    pending: Option<Pending>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    from: String,
    to: String,
    /// The rollback was already attempted: never loop.
    rolled_back: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    pub version: String,
    pub notes: Option<String>,
}

#[derive(Default)]
pub struct UpdaterState {
    found: Mutex<Option<(Available, Update)>>,
}

fn state_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_local_data_dir().ok().map(|d| d.join("update-state.json"))
}

fn load(app: &AppHandle) -> Persisted {
    state_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, s: &Persisted) {
    if let Some(p) = state_path(app) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(tmp, p);
        }
    }
}

fn bucket(app: &AppHandle) -> u8 {
    let mut s = load(app);
    if let Some(b) = s.bucket {
        return b;
    }
    let mut r = [0u8; 1];
    let b = if getrandom::fill(&mut r).is_ok() { r[0] % 100 } else { 0 };
    s.bucket = Some(b);
    save(app, &s);
    b
}

/// Is this install inside the release's rollout percentage? (pure, tested)
pub fn in_rollout(bucket: u8, raw_manifest: &serde_json::Value) -> bool {
    let pct = raw_manifest
        .get("rollout")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(100)
        .min(100);
    u64::from(bucket) < pct
}

/// Signed manifest of one specific release (used for rollback).
pub fn release_manifest_url(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{version}/latest.json")
}

fn current(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

async fn check(app: &AppHandle) -> Result<Option<(Available, Update)>, String> {
    let Some(update) = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if !in_rollout(bucket(app), &update.raw_json) {
        return Ok(None); // staged: not this PC's turn yet
    }
    let a = Available { version: update.version.clone(), notes: update.body.clone() };
    Ok(Some((a, update)))
}

/// After an update: give the new service time to come up; if it never does, go back.
async fn verify_or_roll_back(app: &AppHandle, relay: &Arc<Relay>) {
    let mut s = load(app);
    let Some(p) = s.pending.clone() else { return };
    if p.to != current(app) || p.rolled_back {
        // Not running the version we installed (the user reinstalled something else),
        // or we already rolled back once: stop tracking.
        s.pending = None;
        save(app, &s);
        return;
    }
    let deadline = std::time::Instant::now() + HEALTH_WAIT;
    while std::time::Instant::now() < deadline {
        if relay.up.load(Ordering::SeqCst) {
            s.pending = None;
            save(app, &s);
            return; // healthy
        }
        tokio_sleep(Duration::from_secs(2)).await;
    }
    // The new version's service never answered: reinstall the previous signed release.
    s.pending = Some(Pending { rolled_back: true, ..p.clone() });
    save(app, &s);
    let _ = app.emit("update://rollback", &p.from);
    let result: Result<(), String> = async {
        let url = release_manifest_url(&p.from).parse().map_err(|e: url::ParseError| e.to_string())?;
        let updater = app
            .updater_builder()
            .endpoints(vec![url])
            .map_err(|e| e.to_string())?
            // Allow installing an *older* version: that's the point of a rollback.
            .version_comparator(|cur, upd| upd.version != cur)
            .build()
            .map_err(|e| e.to_string())?;
        let up = updater.check().await.map_err(|e| e.to_string())?.ok_or("previous release not found")?;
        up.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())
    }
    .await;
    if let Err(e) = result {
        let _ = app.emit("update://rollback_failed", e);
    }
}

async fn tokio_sleep(d: Duration) {
    tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await.ok();
}

/// Background task: health check of a fresh update, then a check every 12 h.
pub fn start(app: AppHandle, relay: Arc<Relay>) {
    if cfg!(debug_assertions) {
        return; // dev builds never update themselves
    }
    tauri::async_runtime::spawn(async move {
        verify_or_roll_back(&app, &relay).await;
        loop {
            if let Ok(Some((a, u))) = check(&app).await {
                let _ = app.emit("update://available", &a);
                *app.state::<UpdaterState>().found.lock().unwrap() = Some((a, u));
            }
            tokio_sleep(CHECK_EVERY).await;
        }
    });
}

/// What the UI should show (the event may have fired before the window existed).
#[tauri::command]
pub fn update_status(st: State<'_, UpdaterState>) -> Option<Available> {
    st.found.lock().unwrap().as_ref().map(|(a, _)| a.clone())
}

/// The user pressed "Update": remember where we came from, then download, verify the
/// signature and run the installer (Windows asks for administrator rights).
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    let found = app.state::<UpdaterState>().found.lock().unwrap().take();
    let Some((a, update)) = found else {
        return Err("no update available".into());
    };
    let mut s = load(&app);
    s.pending = Some(Pending { from: current(&app), to: a.version.clone(), rolled_back: false });
    save(&app, &s);
    let res = update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string());
    if res.is_err() {
        let mut s = load(&app);
        s.pending = None; // nothing was installed
        save(&app, &s);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn staged_rollout_by_bucket() {
        assert!(in_rollout(0, &json!({ "version": "1.0.1" })), "no field = everyone");
        assert!(in_rollout(99, &json!({ "rollout": 100 })));
        assert!(in_rollout(9, &json!({ "rollout": 10 })));
        assert!(!in_rollout(10, &json!({ "rollout": 10 })));
        assert!(!in_rollout(0, &json!({ "rollout": 0 })), "0 halts the release");
        assert!(in_rollout(99, &json!({ "rollout": 250 })), "clamped to 100");
        // Over many installs, a 25 % rollout reaches ~25 % of buckets.
        assert_eq!((0..100u8).filter(|b| in_rollout(*b, &json!({ "rollout": 25 }))).count(), 25);
    }

    #[test]
    fn rollback_manifest_is_the_signed_manifest_of_that_release() {
        assert_eq!(
            release_manifest_url("1.0.0-alpha.1"),
            "https://github.com/monteero13/overdrive-optimizer/releases/download/v1.0.0-alpha.1/latest.json"
        );
    }
}
