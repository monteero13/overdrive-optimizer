//! Small persistent state next to the journal (`state.json`) plus one file per session
//! report. The journal stays the source of truth for every change made to the system;
//! this holds preferences and UI-facing history only.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use overdrive_core::coach::CoachFlow;
use serde::{Deserialize, Serialize};

use crate::proto::{PersistentChangeView, Prompt, ReportSummary, SessionReport, Settings};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachRecord {
    pub flow: CoachFlow,
    /// started | verified_applied | not_applied | watching | done
    pub state: String,
    /// The user rebooted into the BIOS from Overdrive: verify on the next start.
    pub pending_verify: bool,
    pub watch_from: Option<i64>,
    pub watch_until: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Store {
    pub settings: Settings,
    /// gameId → tweakId → enabled (only explicit user choices).
    pub toggles: BTreeMap<String, BTreeMap<String, bool>>,
    /// gameId → full path of the executable, learned when the game first ran.
    pub learned_exe: BTreeMap<String, String>,
    /// Games the user asked to optimize: their sessions follow the A/B plan.
    pub opted_in: BTreeSet<String>,
    pub prompts: Vec<Prompt>,
    pub coach: Option<CoachRecord>,
    pub bios_changes: Vec<PersistentChangeView>,
    /// "YYYY-MM" → drift repairs that month.
    pub drift_repaired: BTreeMap<String, u32>,
    pub last_windows_build: Option<u32>,
    pub last_gpu_driver: Option<String>,
    pub reports: Vec<ReportSummary>,
    pub whitelist: BTreeSet<String>,
    pub next_id: u64,
}

impl Store {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Write-then-rename, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, path)
    }

    pub fn new_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}_{}", self.next_id)
    }
}

pub struct Reports {
    dir: PathBuf,
}

impl Reports {
    pub fn new(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        Self { dir }
    }
    fn path(&self, tx: i64) -> PathBuf {
        self.dir.join(format!("{tx}.json"))
    }
    pub fn put(&self, r: &SessionReport) -> io::Result<()> {
        fs::write(self.path(r.tx_id), serde_json::to_vec(r)?)
    }
    pub fn get(&self, tx: i64) -> Option<SessionReport> {
        serde_json::from_str(&fs::read_to_string(self.path(tx)).ok()?).ok()
    }
    /// Downsampled frametimes of every measured session (baseline charts need them).
    pub fn put_series(&self, session: i64, v: &[f64]) -> io::Result<()> {
        fs::write(self.dir.join(format!("series-{session}.json")), serde_json::to_vec(v)?)
    }
    pub fn get_series(&self, session: i64) -> Option<Vec<f64>> {
        serde_json::from_str(&fs::read_to_string(self.dir.join(format!("series-{session}.json"))).ok()?).ok()
    }
}
