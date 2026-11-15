//! Module C — the optimization catalog (goals.md §5.3).
//!
//! Metadata lives in `catalog/tweaks.toml` (embedded at build time). Each id has a
//! *builder* here that reads the current system state and returns the exact actions —
//! or says why it doesn't apply / is already optimal. Builders never touch the game process.

use serde::{Deserialize, Serialize};

use crate::engine::{
    Action, RegData, RegRoot, RegTarget, StartType, SysError, SysResult, SystemOps,
};
use crate::profile::{HardwareProfile, PowerSource};
use crate::safety::{AntiCheat, ProcInfo, SafetyPolicy};

const CATALOG_TOML: &str = include_str!("../../../catalog/tweaks.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Session,
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    BeforeLaunch,
    AtLaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Proven,
    VersionDependent,
    Disputed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    Frametime,
    BackgroundNoise,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TweakMeta {
    pub id: String,
    pub scope: Scope,
    pub timing: Timing,
    pub risk: Risk,
    pub evidence: Evidence,
    pub anticheat_safe: bool,
    pub measure: Measure,
    pub requires_reboot: bool,
    pub persistent_candidate: bool,
    pub order: u32,
    #[serde(default)]
    pub requires: Vec<String>,
    pub notes: String,
}

impl TweakMeta {
    /// Easy mode only ever uses low-risk, proven changes.
    pub fn easy_mode_ok(&self) -> bool {
        self.risk == Risk::Low && self.evidence == Evidence::Proven
    }
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    tweak: Vec<TweakMeta>,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub tweaks: Vec<TweakMeta>,
}

/// Who/what the plan is for.
#[derive(Debug, Clone)]
pub struct GameTarget {
    pub id: String,
    /// Full path of the game executable (UserGpuPreferences is keyed by path).
    pub exe_path: String,
    /// File name, e.g. `FortniteClient-Win64-Shipping.exe` (IFEO is keyed by name).
    pub exe_name: String,
    pub anticheat: AntiCheat,
}

pub struct PlanContext<'a> {
    pub game: &'a GameTarget,
    pub hw: &'a HardwareProfile,
    /// The interactive user's hive (the service runs as LocalSystem).
    pub user_root: RegRoot,
    /// Process list — only known at launch. `None` while planning/arming.
    pub procs: Option<&'a [ProcInfo]>,
    pub policy: &'a SafetyPolicy,
    pub capabilities: &'a [String],
    pub now_hour: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Build {
    Actions(Vec<Action>),
    /// Doesn't apply to this machine/game (i18n reason key).
    NotApplicable(&'static str),
    /// Already in the desired state: nothing to do, nothing to test.
    AlreadyOptimal,
    /// Depends on launch-time information (process list).
    Deferred,
}

impl Catalog {
    pub fn builtin() -> Self {
        Self::parse(CATALOG_TOML).expect("embedded catalog/tweaks.toml is valid")
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let f: CatalogFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut tweaks = f.tweak;
        tweaks.sort_by_key(|t| t.order);
        Ok(Self { tweaks })
    }

    pub fn get(&self, id: &str) -> Option<&TweakMeta> {
        self.tweaks.iter().find(|t| t.id == id)
    }

    pub fn build(&self, id: &str, ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
        let meta = self
            .get(id)
            .ok_or_else(|| SysError::NotFound(format!("tweak {id}")))?;
        if meta.requires.iter().any(|r| !ctx.capabilities.contains(r)) {
            return Ok(Build::NotApplicable("needs_driver_profiles"));
        }
        let builder = BUILDERS
            .iter()
            .find(|(b, _)| *b == id)
            .map(|(_, f)| f)
            .ok_or_else(|| SysError::NotFound(format!("builder {id}")))?;
        builder(ctx, sys)
    }
}

type Builder = fn(&PlanContext, &dyn SystemOps) -> SysResult<Build>;

/// One builder per catalog id. `catalog_and_builders_match` (tests) keeps both in sync.
pub const BUILDERS: &[(&str, Builder)] = &[
    ("hybrid_gpu", hybrid_gpu),
    ("frame_cap", frame_cap),
    ("power_session", power_session),
    ("gamedvr_off", gamedvr_off),
    ("background_isolation", background_isolation),
    ("game_priority", game_priority),
    ("background_noise", background_noise),
    ("game_mode", game_mode),
    ("windowed_opt", windowed_opt),
    ("hags", hags),
    ("pause_updates", pause_updates),
];

// ---------------------------------------------------------------- helpers

/// Set `key=value` inside a `a=1;b=2;` style string, keeping every other flag.
pub fn merge_flag(current: &str, key: &str, value: &str) -> String {
    let mut parts: Vec<String> = current
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    match parts.iter_mut().find(|p| p.split('=').next() == Some(key)) {
        Some(p) => *p = format!("{key}={value}"),
        None => parts.push(format!("{key}={value}")),
    }
    parts.join(";") + ";"
}

fn flag_is(current: &str, key: &str, value: &str) -> bool {
    current
        .split(';')
        .map(str::trim)
        .any(|p| p == format!("{key}={value}"))
}

fn reg_string(
    sys: &dyn SystemOps,
    root: &RegRoot,
    key: &str,
    name: &str,
) -> SysResult<Option<String>> {
    Ok(match sys.reg_read(root, key, name)? {
        Some(RegData::String(s)) => Some(s),
        _ => None,
    })
}

fn reg_dword(sys: &dyn SystemOps, root: &RegRoot, key: &str, name: &str) -> SysResult<Option<u32>> {
    Ok(match sys.reg_read(root, key, name)? {
        Some(RegData::Dword(v)) => Some(v),
        _ => None,
    })
}

fn set(root: &RegRoot, key: &str, name: &str, data: RegData) -> Action {
    Action::RegistryValue {
        root: root.clone(),
        key: key.into(),
        name: name.into(),
        target: RegTarget::Set(data),
    }
}

fn done(actions: Vec<Action>) -> Build {
    if actions.is_empty() {
        Build::AlreadyOptimal
    } else {
        Build::Actions(actions)
    }
}

pub const GPU_PREFS_KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
pub const IFEO_KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";
pub const HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
pub const ULTIMATE_PERFORMANCE: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";
pub const TELEMETRY_TASKS: &[&str] = &[
    r"\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser",
    r"\Microsoft\Windows\Application Experience\ProgramDataUpdater",
    r"\Microsoft\Windows\Customer Experience Improvement Program\Consolidator",
    r"\Microsoft\Windows\Customer Experience Improvement Program\UsbCeip",
];

// ---------------------------------------------------------------- builders

fn hybrid_gpu(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    let hybrid =
        ctx.hw.is_laptop() && ctx.hw.dedicated_gpu().is_some() && ctx.hw.integrated_gpu().is_some();
    if !hybrid {
        return Ok(Build::NotApplicable("no_hybrid_gpu"));
    }
    let cur =
        reg_string(sys, &ctx.user_root, GPU_PREFS_KEY, &ctx.game.exe_path)?.unwrap_or_default();
    if flag_is(&cur, "GpuPreference", "2") {
        return Ok(Build::AlreadyOptimal);
    }
    Ok(Build::Actions(vec![set(
        &ctx.user_root,
        GPU_PREFS_KEY,
        &ctx.game.exe_path,
        RegData::String(merge_flag(&cur, "GpuPreference", "2")),
    )]))
}

fn frame_cap(_: &PlanContext, _: &dyn SystemOps) -> SysResult<Build> {
    // Reached only when the "driver_profiles" capability exists (Phase 7).
    Ok(Build::NotApplicable("needs_driver_profiles"))
}

fn power_session(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    if ctx.hw.power.power_source.val() == Some(PowerSource::Battery) {
        return Ok(Build::NotApplicable("on_battery"));
    }
    let schemes = sys.power_schemes()?;
    let target = [HIGH_PERFORMANCE, ULTIMATE_PERFORMANCE]
        .into_iter()
        .find(|g| schemes.iter().any(|s| s == g));
    let Some(target) = target else {
        return Ok(Build::NotApplicable("no_high_performance_plan"));
    };
    if [HIGH_PERFORMANCE, ULTIMATE_PERFORMANCE].contains(&sys.active_power_scheme()?.as_str()) {
        return Ok(Build::AlreadyOptimal);
    }
    Ok(Build::Actions(vec![Action::PowerScheme {
        guid: target.into(),
    }]))
}

fn gamedvr_off(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    let r = &ctx.user_root;
    let mut a = Vec::new();
    // Both default to "on" when absent.
    if reg_dword(sys, r, r"System\GameConfigStore", "GameDVR_Enabled")?.unwrap_or(1) != 0 {
        a.push(set(
            r,
            r"System\GameConfigStore",
            "GameDVR_Enabled",
            RegData::Dword(0),
        ));
    }
    if reg_dword(
        sys,
        r,
        r"Software\Microsoft\Windows\CurrentVersion\GameDVR",
        "AppCaptureEnabled",
    )?
    .unwrap_or(1)
        != 0
    {
        a.push(set(
            r,
            r"Software\Microsoft\Windows\CurrentVersion\GameDVR",
            "AppCaptureEnabled",
            RegData::Dword(0),
        ));
    }
    Ok(done(a))
}

fn background_isolation(ctx: &PlanContext, _: &dyn SystemOps) -> SysResult<Build> {
    match ctx.procs {
        None => Ok(Build::Deferred),
        Some(procs) => Ok(done(ctx.policy.background_actions(procs))),
    }
}

fn game_priority(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    let key = format!(r"{IFEO_KEY}\{}\PerfOptions", ctx.game.exe_name);
    let m = RegRoot::LocalMachine;
    let mut a = Vec::new();
    if reg_dword(sys, &m, &key, "CpuPriorityClass")? != Some(3) {
        a.push(set(&m, &key, "CpuPriorityClass", RegData::Dword(3))); // High
    }
    if reg_dword(sys, &m, &key, "IoPriority")? != Some(3) {
        a.push(set(&m, &key, "IoPriority", RegData::Dword(3))); // High
    }
    Ok(done(a))
}

fn background_noise(_: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    let mut a = Vec::new();
    match sys.service_start_type("DiagTrack") {
        Ok(StartType::Automatic) => a.push(Action::ServiceStart {
            name: "DiagTrack".into(),
            start: StartType::Manual,
        }),
        Ok(_) | Err(SysError::NotFound(_)) => {}
        Err(e) => return Err(e),
    }
    for t in TELEMETRY_TASKS {
        match sys.task_enabled(t) {
            Ok(true) => a.push(Action::ScheduledTask {
                path: (*t).into(),
                enabled: false,
            }),
            Ok(false) | Err(SysError::NotFound(_)) => {} // not on this build: skip
            Err(e) => return Err(e),
        }
    }
    Ok(done(a))
}

fn game_mode(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    // On by default since Windows 10 1703: only act if the user turned it off.
    match reg_dword(
        sys,
        &ctx.user_root,
        r"Software\Microsoft\GameBar",
        "AutoGameModeEnabled",
    )? {
        Some(0) => Ok(Build::Actions(vec![set(
            &ctx.user_root,
            r"Software\Microsoft\GameBar",
            "AutoGameModeEnabled",
            RegData::Dword(1),
        )])),
        _ => Ok(Build::AlreadyOptimal),
    }
}

fn windowed_opt(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    if ctx.hw.windows.is_windows_11.val() != Some(true) {
        return Ok(Build::NotApplicable("windows_11_only"));
    }
    let cur = reg_string(
        sys,
        &ctx.user_root,
        GPU_PREFS_KEY,
        "DirectXUserGlobalSettings",
    )?
    .unwrap_or_default();
    if flag_is(&cur, "SwapEffectUpgradeEnable", "1") {
        return Ok(Build::AlreadyOptimal);
    }
    Ok(Build::Actions(vec![set(
        &ctx.user_root,
        GPU_PREFS_KEY,
        "DirectXUserGlobalSettings",
        RegData::String(merge_flag(&cur, "SwapEffectUpgradeEnable", "1")),
    )]))
}

fn hags(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    if ctx.hw.dedicated_gpu().is_none() || ctx.hw.windows.build.val().unwrap_or(0) < 19041 {
        return Ok(Build::NotApplicable("hags_unsupported"));
    }
    let key = r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers";
    if reg_dword(sys, &RegRoot::LocalMachine, key, "HwSchMode")? == Some(2) {
        return Ok(Build::AlreadyOptimal);
    }
    Ok(Build::Actions(vec![set(
        &RegRoot::LocalMachine,
        key,
        "HwSchMode",
        RegData::Dword(2),
    )]))
}

fn pause_updates(ctx: &PlanContext, _: &dyn SystemOps) -> SysResult<Build> {
    let key = r"SOFTWARE\Microsoft\WindowsUpdate\UX\Settings";
    let start = ctx.now_hour as u32 % 24;
    let end = (start + 12) % 24; // Windows allows active hours up to 18 h
    Ok(Build::Actions(vec![
        set(
            &RegRoot::LocalMachine,
            key,
            "ActiveHoursStart",
            RegData::Dword(start),
        ),
        set(
            &RegRoot::LocalMachine,
            key,
            "ActiveHoursEnd",
            RegData::Dword(end),
        ),
    ]))
}
