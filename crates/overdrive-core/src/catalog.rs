//! Module C — the optimization catalog (goals.md §5.3).
//!
//! Metadata lives in `catalog/tweaks.toml` (embedded at build time). Each id has a
//! *builder* here that reads the current system state and returns the exact actions —
//! or says why it doesn't apply / is already optimal. Builders never touch the game process.

use serde::{Deserialize, Serialize};

use crate::config::{self, ConfigEdit, ConfigFormat};
use crate::engine::{
    Action, DriverSetting, DriverTarget, RegData, RegRoot, RegTarget, StartType, SysError,
    SysResult, SystemOps,
};
use crate::games::{self, GameDb, Role, UserDirs};
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
    /// Frametime spread (stdev, lower is better): for changes that trade average FPS for evenness.
    Pacing,
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
    /// Per-game config database (catalog/games/*.toml).
    pub games: GameDb,
}

/// What the baseline sessions of a game say (no tweaks active).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Baseline {
    pub avg_fps: f64,
    pub low_1: f64,
    /// Most baseline sessions were CPU-bound (or mixed).
    pub cpu_bound: bool,
    pub runs: usize,
}

impl Baseline {
    /// "Slightly below the sustainable 1 % low" (goals.md §5.3 item 2), in steps of 5 so the
    /// value stays the same while more baseline runs come in. `None` = a cap wouldn't help:
    /// below 60 FPS it would feel worse, and with no headroom it would do nothing.
    pub fn frame_cap(&self) -> Option<u32> {
        let cap = ((self.low_1 * 0.97) / 5.0).floor() * 5.0;
        (cap >= 60.0 && cap <= self.avg_fps * 0.9).then_some(cap as u32)
    }
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
    /// Install folder (`{install}` in game config paths). May be empty.
    pub install_dir: String,
}

/// Generic runtimes: keying anything by these names would change other programs too.
const SHARED_EXES: &[&str] = &["javaw.exe", "java.exe", "python.exe", "pythonw.exe"];

impl GameTarget {
    fn shared_exe(&self, db: &GameDb) -> bool {
        SHARED_EXES.iter().any(|s| s.eq_ignore_ascii_case(&self.exe_name))
            || db.find(self).is_some_and(|g| g.shared_exe)
    }
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
    pub games: &'a GameDb,
    /// The interactive user's folders (game config paths).
    pub dirs: &'a UserDirs,
    pub baseline: Option<Baseline>,
    /// Expert mode: game config settings that are disputed or visibly change the image.
    pub expert: bool,
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
        Ok(Self {
            tweaks,
            games: GameDb::builtin(),
        })
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
    ("driver_power", driver_power),
    ("game_config", game_config),
    ("unreal_engine_ini", unreal_engine_ini),
];

/// One line of an Expert preview diff: what a file patch would change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewLine {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub key: String,
    /// `None` = the key is added.
    pub from: Option<String>,
    pub to: String,
}

/// Preview of every file patch in `actions`, read from the current files.
pub fn preview(actions: &[Action], sys: &dyn SystemOps) -> Vec<PreviewLine> {
    let mut out = Vec::new();
    for a in actions {
        let Action::FilePatch { path, format, edits } = a else {
            continue;
        };
        let bytes = sys.file_read(path).ok().flatten().map(|f| f.bytes).unwrap_or_default();
        let cur = config::read(&bytes, *format, edits).unwrap_or_else(|_| vec![None; edits.len()]);
        for (e, from) in edits.iter().zip(cur) {
            out.push(PreviewLine {
                file: path.clone(),
                section: e.section.clone(),
                key: e.key.clone(),
                from,
                to: e.value.clone(),
            });
        }
    }
    out
}

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

/// A config file of this game, read: `(path, format, bytes)`.
type ReadFile = (String, ConfigFormat, Vec<u8>);

/// Outcome of looking for a game's config file.
enum Found {
    File(ReadFile),
    ReadOnly,
    Missing,
}

fn find_file(ctx: &PlanContext, sys: &dyn SystemOps, template: &str, format: ConfigFormat) -> SysResult<Found> {
    let Some(path) = games::expand(template, ctx.dirs, &ctx.game.install_dir) else {
        return Ok(Found::Missing);
    };
    Ok(match sys.file_read(&path)? {
        None => Found::Missing,
        Some(f) if f.readonly => Found::ReadOnly,
        Some(f) if f.bytes.len() > crate::engine::MAX_PATCH_FILE => Found::Missing,
        Some(f) => Found::File((path, format, f.bytes)),
    })
}

fn patch_action(path: String, format: ConfigFormat, edits: Vec<ConfigEdit>) -> Action {
    Action::FilePatch { path, format, edits }
}

/// The game's own frame limiter key, if the database knows one for this game.
fn game_frame_limiter(ctx: &PlanContext, sys: &dyn SystemOps, cap: u32) -> SysResult<Option<Build>> {
    if !ctx.games.may_patch(ctx.game) {
        return Ok(None);
    }
    let Some(entry) = ctx.games.find(ctx.game) else {
        return Ok(None);
    };
    let Some(s) = entry.setting.iter().find(|s| s.role == Role::FrameCap) else {
        return Ok(None);
    };
    let Some(file) = entry.file(&s.file) else {
        return Ok(None);
    };
    let (path, format, bytes) = match find_file(ctx, sys, &file.path, file.format)? {
        Found::File(f) => f,
        Found::ReadOnly => return Ok(Some(Build::NotApplicable("config_read_only"))),
        Found::Missing => return Ok(None),
    };
    let e = games::edit(s, games::format_value(cap, s.value_format));
    let cur = config::read(&bytes, format, std::slice::from_ref(&e)).unwrap_or_default();
    // The player already caps at or below our value: theirs is at least as even.
    if let Some(c) = cur.first().cloned().flatten().and_then(|v| v.trim().parse::<f64>().ok()) {
        if c > 0.0 && c <= cap as f64 {
            return Ok(Some(Build::AlreadyOptimal));
        }
    }
    Ok(Some(Build::Actions(vec![patch_action(path, format, vec![e])])))
}

fn frame_cap(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    let Some(b) = ctx.baseline else {
        return Ok(Build::NotApplicable("needs_baseline"));
    };
    if !b.cpu_bound {
        return Ok(Build::NotApplicable("not_cpu_bound"));
    }
    let Some(cap) = b.frame_cap() else {
        return Ok(Build::NotApplicable("frame_cap_not_useful"));
    };
    // 1) Driver limiter for the game's executable (NVIDIA): works in every game.
    if sys.driver_profiles() && !ctx.game.exe_name.is_empty() && !ctx.game.shared_exe(ctx.games) {
        let cur = sys.driver_get(&ctx.game.exe_name, DriverSetting::MaxFrameRate)?;
        if cur.is_some_and(|c| c > 0 && c <= cap) {
            return Ok(Build::AlreadyOptimal);
        }
        return Ok(Build::Actions(vec![Action::DriverSetting {
            exe: ctx.game.exe_name.clone(),
            setting: DriverSetting::MaxFrameRate,
            target: DriverTarget::Set(cap),
        }]));
    }
    // 2) The game's own limiter, from the per-game database (never for protected titles).
    if let Some(b) = game_frame_limiter(ctx, sys, cap)? {
        return Ok(b);
    }
    // 3) Nothing we can set safely: the UI recommends the in-game limiter instead.
    Ok(Build::NotApplicable("no_frame_limiter"))
}

fn driver_power(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    if !sys.driver_profiles() {
        return Ok(Build::NotApplicable("no_driver_profiles"));
    }
    if ctx.game.exe_name.is_empty() || ctx.game.shared_exe(ctx.games) {
        return Ok(Build::NotApplicable("shared_executable"));
    }
    // Laptop on battery: maximum clocks would only drain it.
    if ctx.hw.is_laptop() && ctx.hw.power.power_source.val() == Some(PowerSource::Battery) {
        return Ok(Build::NotApplicable("on_battery"));
    }
    if sys.driver_get(&ctx.game.exe_name, DriverSetting::PowerMode)? == Some(1) {
        return Ok(Build::AlreadyOptimal);
    }
    Ok(Build::Actions(vec![Action::DriverSetting {
        exe: ctx.game.exe_name.clone(),
        setting: DriverSetting::PowerMode,
        target: DriverTarget::Set(1), // PREFERRED_PSTATE_PREFER_MAX
    }]))
}

/// The per-game recommended settings (catalog/games/*.toml).
fn game_config(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    let Some(entry) = ctx.games.find(ctx.game) else {
        return Ok(Build::NotApplicable("no_game_config"));
    };
    if !ctx.games.may_patch(ctx.game) {
        return Ok(Build::NotApplicable("game_protected"));
    }
    let (mut actions, mut found, mut read_only) = (Vec::new(), false, false);
    for file in &entry.file {
        let wanted: Vec<_> = entry
            .setting
            .iter()
            .filter(|s| s.file == file.id && s.role == Role::Fixed)
            .filter(|s| ctx.expert || s.easy_mode_ok())
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let (path, format, bytes) = match find_file(ctx, sys, &file.path, file.format)? {
            Found::File(f) => f,
            Found::ReadOnly => {
                read_only = true;
                continue;
            }
            Found::Missing => continue,
        };
        found = true;
        let edits: Vec<ConfigEdit> = wanted
            .iter()
            .map(|s| games::edit(s, s.value.clone().unwrap_or_default()))
            .collect();
        let Ok(cur) = config::read(&bytes, format, &edits) else {
            continue; // not text we can edit safely
        };
        let edits: Vec<ConfigEdit> = wanted
            .iter()
            .zip(edits)
            .zip(cur)
            .filter(|((s, _), c)| s.wants_change(c.as_deref()))
            .map(|((_, e), _)| e)
            .collect();
        if !edits.is_empty() {
            actions.push(patch_action(path, format, edits));
        }
    }
    if !found {
        return Ok(Build::NotApplicable(if read_only { "config_read_only" } else { "config_not_found" }));
    }
    Ok(done(actions))
}

/// Generic Unreal Engine rule (goals.md §5.5): Expert only, with a preview diff. Turns off
/// motion blur, chromatic aberration and volumetric fog through `Engine.ini`, which most
/// Unreal games read from the user's Saved\Config folder.
pub const UNREAL_EDITS: &[(&str, &str)] = &[
    ("r.MotionBlurQuality", "0"),
    ("r.SceneColorFringeQuality", "0"),
    ("r.VolumetricFog", "0"),
];

fn unreal_engine_ini(ctx: &PlanContext, sys: &dyn SystemOps) -> SysResult<Build> {
    if !ctx.games.may_patch(ctx.game) {
        return Ok(Build::NotApplicable("game_protected"));
    }
    let Some(project) = games::unreal_project(&ctx.game.exe_path) else {
        return Ok(Build::NotApplicable("not_unreal"));
    };
    for dir in games::unreal_config_dirs(&project, ctx.dirs) {
        // Only where the game already keeps its settings (GameUserSettings.ini).
        if sys.file_read(&format!(r"{dir}\GameUserSettings.ini"))?.is_none() {
            continue;
        }
        let path = format!(r"{dir}\Engine.ini");
        let bytes = match sys.file_read(&path)? {
            Some(f) if f.readonly => return Ok(Build::NotApplicable("config_read_only")),
            Some(f) => f.bytes,
            // We never create config files: no Engine.ini, nothing to do.
            None => return Ok(Build::NotApplicable("config_not_found")),
        };
        let edits: Vec<ConfigEdit> = UNREAL_EDITS
            .iter()
            .map(|(k, v)| ConfigEdit {
                section: Some("SystemSettings".into()),
                key: (*k).into(),
                value: (*v).into(),
            })
            .collect();
        let Ok(cur) = config::read(&bytes, ConfigFormat::Ini, &edits) else {
            return Ok(Build::NotApplicable("config_not_found"));
        };
        let edits: Vec<ConfigEdit> = edits
            .into_iter()
            .zip(cur)
            .filter(|(e, c)| c.as_deref() != Some(e.value.as_str()))
            .map(|(e, _)| e)
            .collect();
        if edits.is_empty() {
            return Ok(Build::AlreadyOptimal);
        }
        return Ok(Build::Actions(vec![patch_action(path, ConfigFormat::Ini, edits)]));
    }
    Ok(Build::NotApplicable("config_not_found"))
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
    // IFEO is keyed by file name: for javaw.exe it would raise every Java program.
    if ctx.game.exe_name.is_empty() || ctx.game.shared_exe(ctx.games) {
        return Ok(Build::NotApplicable("shared_executable"));
    }
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
