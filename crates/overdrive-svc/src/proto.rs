//! overdrive.v1 wire types, mirroring `ui/web/src/proto/types.ts` field by field.
//! `tests/contract.rs` writes a full snapshot to `ui/web/test/fixtures/svc-snapshot.json`
//! and the UI test suite checks it against the TypeScript shapes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Params = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldView<T> {
    pub value: Option<T>,
    pub confidence: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RamSpeed {
    pub configured: u32,
    pub rated: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareSummary {
    pub tier: u8,
    pub is_laptop: bool,
    pub power_source: &'static str,
    pub os: String,
    pub cpu: FieldView<String>,
    pub gpu: FieldView<String>,
    pub gpu_secondary: Option<FieldView<String>>,
    pub vram_gb: FieldView<f64>,
    pub ram_gb: FieldView<f64>,
    pub ram_speed: FieldView<RamSpeed>,
    pub ram_channels: FieldView<u8>,
    pub storage: FieldView<String>,
    pub storage_free_pct: FieldView<f64>,
    pub board_vendor: FieldView<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PulseState {
    Idle,
    Bottleneck { label: String, intensity: f64 },
    Thermal { pressure: f64 },
    Session { label: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    pub low1_delta_pct: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub runs: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameCard {
    pub id: String,
    pub name: String,
    pub launcher: &'static str,
    pub anti_cheat: &'static str,
    pub anti_cheat_name: Option<String>,
    pub bottleneck: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottleneck_params: Option<Params>,
    pub status: &'static str,
    pub fixes_ready: usize,
    pub measured: Option<Measured>,
    pub hue: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub game_id: String,
    pub started_at: i64,
    pub fps: f64,
    pub low1: f64,
    pub frametime_ms: f64,
    pub gpu_busy_ms: f64,
    pub recent: Vec<f64>,
    pub bottleneck: String,
    pub tweaks_active: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationView {
    pub op_id: String,
    pub method: String,
    pub label_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_params: Option<Params>,
    pub step: usize,
    pub total: usize,
    pub step_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_params: Option<Params>,
    pub started_at: i64,
    pub deadline_ms: i64,
    pub cancellable: bool,
    pub blocking: bool,
    pub state: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistentChangeView {
    pub tweak_id: String,
    pub since: i64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SuspendedView {
    pub image: String,
    pub action: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Prompt {
    KeepPersistent {
        prompt_id: String,
        tweak_id: String,
        game_id: String,
        runs: usize,
        delta_pct: f64,
    },
    Recovered {
        prompt_id: String,
    },
    Drift {
        prompt_id: String,
        tweak_id: String,
        cause: String,
        repaired: bool,
    },
    BiosVerified {
        prompt_id: String,
        flow_id: String,
        before: u32,
        after: u32,
    },
    Notice {
        prompt_id: String,
        message_key: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        params: Option<Params>,
    },
}

impl Prompt {
    pub fn id(&self) -> &str {
        match self {
            Prompt::KeepPersistent { prompt_id, .. }
            | Prompt::Recovered { prompt_id }
            | Prompt::Drift { prompt_id, .. }
            | Prompt::BiosVerified { prompt_id, .. }
            | Prompt::Notice { prompt_id, .. } => prompt_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub tx_id: i64,
    pub game_id: String,
    pub ended_at: i64,
    pub low1_delta_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSummary {
    pub month: String,
    pub kept_fixed: usize,
    pub drift_repaired: u32,
    pub sessions_measured: usize,
    pub recommendation_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachStepView {
    pub kind: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub params: Option<Params>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachState {
    pub flow_id: String,
    pub state: String,
    pub steps: Vec<CoachStepView>,
    pub watch_until: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub mode: String,
    pub locale: String,
    pub theme: String,
    pub reduce_motion: String,
    pub telemetry_opt_in: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: "easy".into(),
            locale: "es".into(),
            theme: "auto".into(),
            reduce_motion: "auto".into(),
            telemetry_opt_in: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub seq: u64,
    pub boot_id: String,
    pub settings: Settings,
    pub hardware: Option<HardwareSummary>,
    pub issues: Vec<Value>,
    pub pulse: PulseState,
    pub games: Vec<GameCard>,
    pub active_session: Option<SessionView>,
    pub operations: Vec<OperationView>,
    pub persistent_changes: Vec<PersistentChangeView>,
    pub suspended: Vec<SuspendedView>,
    pub prompts: Vec<Prompt>,
    pub reports: Vec<ReportSummary>,
    pub health: Option<HealthSummary>,
    pub coach: Option<CoachState>,
}

// ---- plan preview (Layer 2)

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub tweak_id: String,
    pub scope: String,
    pub risk: String,
    pub evidence: String,
    pub requires_reboot: bool,
    pub enabled: bool,
    pub lock: Option<&'static str>,
    pub history: &'static str,
    pub technical: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreview {
    pub game_id: String,
    pub items: Vec<PlanItem>,
    pub hidden_in_easy: usize,
}

// ---- Layer 3

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub avg_fps: f64,
    pub low1: f64,
    pub low01: f64,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub stdev_ms: f64,
    pub stutters: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TweakVerdict {
    pub tweak_id: String,
    pub verdict: String,
    pub delta_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReport {
    pub tx_id: i64,
    pub game_id: String,
    pub ended_at: i64,
    pub gameplay_secs: f64,
    pub bottleneck: String,
    pub baseline_runs: usize,
    pub baseline: Metrics,
    pub optimized: Metrics,
    pub baseline_series: Vec<f64>,
    pub optimized_series: Vec<f64>,
    pub tweaks: Vec<TweakVerdict>,
}

// ---- wire

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    pub code: String,
    pub message_key: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub details: Option<Params>,
}

impl ErrorBody {
    pub fn new(code: &str, key: &str) -> Self {
        Self {
            code: code.into(),
            message_key: key.into(),
            details: None,
        }
    }
    pub fn with(mut self, k: &str, v: impl Into<Value>) -> Self {
        self.details
            .get_or_insert_with(BTreeMap::new)
            .insert(k.into(), v.into());
        self
    }
}

/// A request as it arrives. Unknown top-level fields are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Req {
    pub kind: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Every method the service answers (ipc-protocol.md §3, closed list).
pub const METHODS: &[&str] = &[
    "hello",
    "state.snapshot",
    "hardware.rescan",
    "plan.preview",
    "plan.setToggle",
    "session.optimize",
    "session.benchmark",
    "persistent.accept",
    "persistent.revert",
    "tx.revertAll",
    "background.resumeAll",
    "background.setWhitelist",
    "bios.startFlow",
    "bios.rebootToFirmware",
    "report.session",
    "report.health",
    "prompt.answer",
    "settings.get",
    "settings.set",
    "op.cancel",
];

pub fn params_of(v: Value) -> Option<Params> {
    match v {
        Value::Object(m) if !m.is_empty() => Some(m.into_iter().collect()),
        _ => None,
    }
}
