//! The service's single-threaded core: owns the journal, the system port, the catalog and
//! every piece of state the UI can see. Everything that happens (a request, a game starting,
//! a timer) goes through here, and after each event the snapshot is rebuilt and only the
//! top-level keys that changed are pushed as an RFC 7396 merge patch.
//!
//! Operations follow ipc-protocol.md §4: the request is answered at once with an `opId`,
//! the operation shows up in `snapshot.operations`, and it ALWAYS ends with `op.finished`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use overdrive_core::catalog::{Build, Catalog, GameTarget, Measure, PlanContext};
use overdrive_core::coach::{self, VerifyStatus};
use overdrive_core::drift::{DriftGuardian, Finding, RepairOutcome, Trigger};
use overdrive_core::engine::journal::{Journal, StepState, TxKind, TxState};
use overdrive_core::engine::tx::{Engine, TxError};
use overdrive_core::engine::{ProcRef, RegRoot, SystemOps};
use overdrive_core::experiment::{self, ExperimentSettings, Verdict};
use overdrive_core::issues::{self, FixPath, Impact, StructuralIssue};
use overdrive_core::profile::{
    Confidence, HardwareProfile, MediaType, PowerSource, Tier,
};
use overdrive_core::safety::{anticheat_status, AcKind, AntiCheat, GameInfo, ProcInfo, SafetyPolicy};
use overdrive_core::session::{ActiveSession, ArmedSession, Exclusion, Orchestrator};
use overdrive_core::telemetry::classify::{classify, Thresholds};
use overdrive_core::telemetry::metrics::{compute, percentile, SessionMetrics};
use overdrive_core::telemetry::FrameSample;
use serde_json::{json, Map, Value};

use crate::library::{is_helper_exe, norm, under};
use crate::platform::{Capture, LibraryGame, Platform};
use crate::proto::*;
use crate::store::{CoachRecord, Reports, Store};

pub type ConnId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Reply(ConnId, String),
    Broadcast(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub svc_version: String,
    /// Reboot-to-firmware is only allowed for the installed service, never in dev/dry-run.
    pub allow_reboot: bool,
}

const WATCH_MS: i64 = 2_000;
const HUD_MS: i64 = 500;
const DRIFT_MS: i64 = 6 * 3_600_000;
const LIBRARY_MS: i64 = 10 * 60_000;
const USER_MS: i64 = 30_000;
const REFRESH_MS: i64 = 60_000;
const WATCH_DAYS_MS: i64 = 3 * 86_400_000;

#[derive(Debug)]
enum Job {
    Optimize(String),
    Benchmark(String),
    Persist { tweak: String, game: String },
    RevertOne(String),
    RevertAll,
    Rescan,
    RebootFw,
}

struct Live {
    game_id: String,
    target: GameTarget,
    proc: ProcRef,
    active: Option<ActiveSession>,
    capture: Option<Box<dyn Capture>>,
    capture_error: Option<String>,
    frames: Vec<FrameSample>,
    view: SessionView,
}

type Fail = ErrorBody;

fn bad(code: &str, key: &str) -> Fail {
    ErrorBody::new(code, key)
}

fn tx_fail(e: TxError) -> Fail {
    match e {
        TxError::StepFailed { tweak_id, .. } => {
            bad("STEP_FAILED", "err.step_failed").with("step", tweak_id)
        }
        other => bad("INTERNAL", "err.internal").with("detail", other.to_string()),
    }
}

fn r1(v: f64) -> f64 {
    if v.is_finite() {
        (v * 10.0).round() / 10.0
    } else {
        0.0
    }
}

fn conf(c: Confidence) -> &'static str {
    match c {
        Confidence::Exact => "exact",
        Confidence::Inferred => "inferred",
        Confidence::Unknown => "unknown",
    }
}

fn fv<T: Clone>(f: &overdrive_core::profile::Field<T>) -> FieldView<T> {
    FieldView {
        value: f.value.clone(),
        confidence: if f.value.is_some() { conf(f.confidence) } else { "unknown" },
    }
}

fn ac_name(k: AcKind) -> &'static str {
    match k {
        AcKind::Vanguard => "Riot Vanguard",
        AcKind::EasyAntiCheat => "Easy Anti-Cheat",
        AcKind::BattlEye => "BattlEye",
        AcKind::Ricochet => "Ricochet",
        AcKind::Faceit => "FACEIT",
        AcKind::EaJavelin => "EA Javelin",
        AcKind::GameGuard => "GameGuard",
        AcKind::Xigncode => "XIGNCODE3",
        AcKind::Vac => "VAC",
        AcKind::PunkBuster => "PunkBuster",
    }
}

fn metrics_view(m: &SessionMetrics) -> Metrics {
    Metrics {
        avg_fps: r1(m.avg_fps),
        low1: r1(m.low_1),
        low01: r1(m.low_01),
        p50_ms: r1(m.p50_ms),
        p99_ms: r1(m.p99_ms),
        stdev_ms: (m.stdev_ms * 100.0).round() / 100.0,
        stutters: m.stutters,
    }
}

fn mean_metrics(v: &[&SessionMetrics]) -> Option<Metrics> {
    if v.is_empty() {
        return None;
    }
    let n = v.len() as f64;
    let avg = |f: &dyn Fn(&SessionMetrics) -> f64| v.iter().map(|m| f(m)).sum::<f64>() / n;
    Some(Metrics {
        avg_fps: r1(avg(&|m| m.avg_fps)),
        low1: r1(avg(&|m| m.low_1)),
        low01: r1(avg(&|m| m.low_01)),
        p50_ms: r1(avg(&|m| m.p50_ms)),
        p99_ms: r1(avg(&|m| m.p99_ms)),
        stdev_ms: (avg(&|m| m.stdev_ms) * 100.0).round() / 100.0,
        stutters: avg(&|m| m.stutters as f64).round() as u32,
    })
}

/// Bucket means: a few hundred points for the report chart.
pub fn downsample(v: &[f64], n: usize) -> Vec<f64> {
    if v.len() <= n {
        return v.iter().map(|x| (x * 100.0).round() / 100.0).collect();
    }
    (0..n)
        .map(|i| {
            let a = i * v.len() / n;
            let b = ((i + 1) * v.len() / n).max(a + 1);
            let m = v[a..b].iter().sum::<f64>() / (b - a) as f64;
            (m * 100.0).round() / 100.0
        })
        .collect()
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut v = v.to_vec();
    v.sort();
    v
}

fn hue(id: &str) -> u32 {
    id.bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
        % 360
}

/// "YYYY-MM" for a unix-ms timestamp (UTC).
pub fn month_of(ms: i64) -> String {
    overdrive_win::civil_from_days(ms.div_euclid(86_400_000))[..7].to_string()
}

pub struct Core {
    cfg: Config,
    journal: Journal,
    sys: Box<dyn SystemOps>,
    platform: Box<dyn Platform>,
    catalog: Catalog,
    store: Store,
    reports: Reports,
    settings: ExperimentSettings,
    user_root: RegRoot,
    profile: Option<HardwareProfile>,
    issues: Vec<StructuralIssue>,
    library: Vec<LibraryGame>,
    armed: HashMap<String, ArmedSession>,
    live: Option<Live>,
    ops: Vec<OperationView>,
    queue: VecDeque<(String, Job)>,
    seq: u64,
    last: Map<String, Value>,
    out: Vec<Out>,
    games_cache: Option<Vec<GameCard>>,
    /// (pid, start_time) → game id or None (not a game): image paths are looked up once.
    seen: HashMap<(u32, u64), Option<String>>,
    t_watch: i64,
    t_hud: i64,
    t_drift: i64,
    t_library: i64,
    t_user: i64,
    t_refresh: i64,
    pub log: Vec<String>,
}

impl Core {
    pub fn new(
        cfg: Config,
        sys: Box<dyn SystemOps>,
        mut platform: Box<dyn Platform>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&cfg.data_dir).map_err(|e| e.to_string())?;
        let journal = Journal::open(&cfg.data_dir.join("journal.db"), &cfg.svc_version)
            .map_err(|e| e.to_string())?;
        let store = Store::load(&cfg.data_dir.join("state.json"));
        let reports = Reports::new(cfg.data_dir.join("reports"));
        let user_root = platform.user_root();
        let now = platform.now_ms();
        let mut core = Self {
            cfg,
            journal,
            sys,
            platform,
            catalog: Catalog::builtin(),
            store,
            reports,
            settings: ExperimentSettings::default(),
            user_root,
            profile: None,
            issues: vec![],
            library: vec![],
            armed: HashMap::new(),
            live: None,
            ops: vec![],
            queue: VecDeque::new(),
            seq: 0,
            last: Map::new(),
            out: vec![],
            games_cache: None,
            seen: HashMap::new(),
            t_watch: 0,
            t_hud: 0,
            t_drift: now,
            t_library: now,
            t_user: now,
            t_refresh: now,
            log: vec![],
        };
        core.startup();
        Ok(core)
    }

    fn note(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if self.log.len() > 500 {
            self.log.drain(..100);
        }
        self.log.push(m);
    }

    fn save(&mut self) {
        if let Err(e) = self.store.save(&self.cfg.data_dir.join("state.json")) {
            self.note(format!("state.json not saved: {e}"));
        }
    }

    // ------------------------------------------------------------------ startup

    /// Journal recovery runs before IPC is accepted (transaction-engine.md §4).
    fn startup(&mut self) {
        let mut notes = Vec::new();
        let engine = Engine::new(&self.journal, self.sys.as_ref());
        let mut recovered = match engine.recover() {
            Ok(r) => r.reverted.len(),
            Err(e) => {
                notes.push(format!("recover failed: {e}"));
                0
            }
        };
        // No game session survives a service restart: anything still applied for a
        // session (an armed game, a crash mid-game) goes back now.
        let leftovers: Vec<i64> = self
            .journal
            .txs_in(&[TxState::Open, TxState::Applying, TxState::Applied])
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.kind == TxKind::Session)
            .map(|t| t.tx_id)
            .collect();
        for tx in leftovers {
            match engine.revert(tx) {
                Ok(_) => recovered += 1,
                Err(e) => notes.push(format!("revert of leftover tx {tx} failed: {e}")),
            }
        }
        drop(engine);
        self.log.extend(notes);
        let _ = self.journal.mark_boot_recovered();
        if recovered > 0 {
            let id = self.store.new_id("p");
            self.store.prompts.push(Prompt::Recovered { prompt_id: id });
        }
        self.rescan();
        self.coach_after_boot();
        self.drift(Trigger::Boot);
        self.save();
    }

    fn rescan(&mut self) {
        let p = self.platform.profile(&self.user_root);
        let mut iss = issues::structural_issues(&p);
        issues::rank(&mut iss);
        self.issues = iss;
        self.profile = Some(p);
        self.library = self.platform.library(&self.user_root);
        self.games_cache = None;
    }

    fn coach_after_boot(&mut self) {
        let Some(mut rec) = self.store.coach.clone() else { return };
        let Some(p) = &self.profile else { return };
        if !rec.pending_verify {
            return;
        }
        rec.pending_verify = false;
        let v = coach::verify_after_reboot(&rec.flow, p);
        let now = self.platform.now_ms();
        let flow_id = rec.flow.flow_id.key().to_string();
        match v.status {
            VerifyStatus::Applied | VerifyStatus::Partial => {
                rec.state = "watching".into();
                rec.watch_from = Some(now);
                rec.watch_until = Some(now + WATCH_DAYS_MS);
                let id = self.store.new_id("p");
                self.store.prompts.push(Prompt::BiosVerified {
                    prompt_id: id,
                    flow_id: flow_id.clone(),
                    before: v.before,
                    after: v.after.unwrap_or(v.before),
                });
                let tweak = format!("bios_{flow_id}");
                self.store.bios_changes.retain(|c| c.tweak_id != tweak);
                self.store.bios_changes.push(PersistentChangeView {
                    tweak_id: tweak,
                    since: now,
                    source: "bios_coach".into(),
                });
            }
            _ => {
                rec.state = "not_applied".into();
                let id = self.store.new_id("p");
                self.store.prompts.push(Prompt::Notice {
                    prompt_id: id,
                    message_key: "notice.bios_not_applied".into(),
                    params: None,
                });
            }
        }
        self.store.coach = Some(rec);
    }

    /// BIOS changes are watched for a few days: any WHEA error, crash or unexpected
    /// shutdown means "go one step down".
    fn coach_watch(&mut self) {
        let Some(mut rec) = self.store.coach.clone() else { return };
        if rec.state != "watching" {
            return;
        }
        let now = self.platform.now_ms();
        let Some(ev) = self
            .platform
            .stability_events(rec.watch_from.unwrap_or(now))
        else {
            return;
        };
        let verdict = coach::assess_stability(&rec.flow, ev);
        let over = rec.watch_until.is_some_and(|u| now >= u);
        if !verdict.stable || over {
            rec.state = if verdict.stable { "done" } else { "started" }.into();
            let mut params = Params::new();
            if let Some(f) = verdict.fallback {
                params.insert("fallback".into(), f.into());
            }
            let id = self.store.new_id("p");
            self.store.prompts.push(Prompt::Notice {
                prompt_id: id,
                message_key: verdict.advice_key.clone(),
                params: (!params.is_empty()).then_some(params),
            });
            self.store.coach = Some(rec);
            self.save();
        }
    }

    fn drift(&mut self, trigger: Trigger) {
        let build = self.profile.as_ref().and_then(|p| p.windows.build.val());
        let driver = self
            .profile
            .as_ref()
            .and_then(|p| p.dedicated_gpu().or(p.gpus.first()))
            .and_then(|g| g.driver_version.value.clone());
        let cause = if driver.is_some() && self.store.last_gpu_driver.is_some() && driver != self.store.last_gpu_driver {
            "driver_update"
        } else {
            "windows_update"
        };
        let g = DriftGuardian {
            journal: &self.journal,
            sys: self.sys.as_ref(),
            catalog: &self.catalog,
        };
        let findings = g.check(trigger).unwrap_or_default();
        let mut repaired = Vec::new();
        let mut errors = Vec::new();
        for f in &findings {
            match g.repair(f, false) {
                Ok(RepairOutcome::Repaired { .. }) => {
                    if let Finding::Reverted { tweak_id, .. } = f {
                        repaired.push(tweak_id.clone());
                    }
                }
                Ok(_) => {}
                Err(e) => errors.push(e.to_string()),
            }
        }
        for e in errors {
            self.note(format!("drift repair failed: {e}"));
        }
        let month = month_of(self.platform.now_ms());
        for t in repaired {
            *self.store.drift_repaired.entry(month.clone()).or_insert(0) += 1;
            let id = self.store.new_id("p");
            self.store.prompts.push(Prompt::Drift {
                prompt_id: id,
                tweak_id: t,
                cause: cause.into(),
                repaired: true,
            });
        }
        self.store.last_windows_build = build.or(self.store.last_windows_build);
        self.store.last_gpu_driver = driver.or(self.store.last_gpu_driver.clone());
        self.games_cache = None;
    }

    // ------------------------------------------------------------------ helpers

    fn game(&self, id: &str) -> Option<&LibraryGame> {
        self.library.iter().find(|g| g.id == id)
    }

    fn target(&self, g: &LibraryGame) -> GameTarget {
        let exe_path = self
            .store
            .learned_exe
            .get(&g.id)
            .cloned()
            .or_else(|| g.exe_hint.clone())
            .unwrap_or_default();
        let exe_name = exe_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let anticheat = anticheat_status(&GameInfo {
            exe: exe_name.clone(),
            install_entries: g.install_entries.clone(),
        });
        GameTarget {
            id: g.id.clone(),
            exe_path,
            exe_name,
            anticheat,
        }
    }

    fn orch(&self, game_id: &str) -> Orchestrator<'_> {
        let mut o = Orchestrator::new(&self.journal, self.sys.as_ref(), &self.catalog);
        o.settings = self.settings;
        o.user_root = self.user_root.clone();
        o.expert = self.store.settings.mode == "expert";
        o.now_hour = self.platform.local_hour();
        o.whitelist = self.store.whitelist.iter().cloned().collect();
        o.disabled = self
            .store
            .toggles
            .get(game_id)
            .map(|m| m.iter().filter(|(_, on)| !**on).map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
        o
    }

    fn baseline_runs(&self, game_id: &str) -> usize {
        self.journal
            .sessions(game_id)
            .unwrap_or_default()
            .iter()
            .filter(|s| s.valid && s.tweak_set.is_empty())
            .count()
    }

    fn persistent_measured(&self) -> Vec<String> {
        let o = Orchestrator::new(&self.journal, self.sys.as_ref(), &self.catalog);
        let mut v: Vec<String> = o
            .persistent_tweaks()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| self.catalog.get(t).is_some_and(|m| m.measure != Measure::None))
            .collect();
        v.sort();
        v
    }

    fn push_notice(&mut self, key: &str, params: Option<Params>) {
        let id = self.store.new_id("p");
        self.store.prompts.push(Prompt::Notice {
            prompt_id: id,
            message_key: key.into(),
            params,
        });
    }

    // ------------------------------------------------------------------ plan

    pub fn plan_preview(&self, game_id: &str) -> Result<PlanPreview, Fail> {
        let g = self.game(game_id).ok_or_else(|| bad("UNKNOWN_GAME", "err.unknown_game"))?;
        let Some(hw) = &self.profile else {
            return Ok(PlanPreview { game_id: game_id.into(), items: vec![], hidden_in_easy: 0 });
        };
        let target = self.target(g);
        let o = self.orch(game_id);
        let (cands, excl) = o.candidates(&target, hw).map_err(tx_fail)?;
        let verdicts = o.verdicts(game_id).map_err(tx_fail)?;
        let persistent = o.persistent_tweaks().map_err(tx_fail)?;
        let expert = o.expert;
        let policy = SafetyPolicy::default();
        let ctx = PlanContext {
            game: &target,
            hw,
            user_root: self.user_root.clone(),
            procs: None,
            policy: &policy,
            capabilities: &[],
            now_hour: o.now_hour,
        };
        let disabled = &o.disabled;
        let mut items = Vec::new();
        let mut hidden = 0;
        for m in &self.catalog.tweaks {
            let (enabled, lock) = if cands.contains(&m.id) {
                (true, None)
            } else if let Some((_, e)) = excl.iter().find(|(id, _)| *id == m.id) {
                match e {
                    Exclusion::UserDisabled | Exclusion::NoBenefit => (false, None),
                    Exclusion::AntiCheat => (false, Some("anticheat")),
                    Exclusion::Blacklisted => (false, Some("blacklisted")),
                    Exclusion::EasyMode => {
                        hidden += 1;
                        continue;
                    }
                    Exclusion::NeedsReboot if !expert => {
                        hidden += 1;
                        continue;
                    }
                    Exclusion::NeedsReboot => (false, None),
                    Exclusion::NotApplicable(_) | Exclusion::AlreadyOptimal => continue,
                }
            } else if m.measure == Measure::None {
                if !expert && !m.easy_mode_ok() {
                    hidden += 1;
                    continue;
                }
                let builds = matches!(
                    self.catalog.build(&m.id, &ctx, self.sys.as_ref()),
                    Ok(Build::Actions(_) | Build::Deferred)
                );
                if !builds {
                    continue;
                }
                (!disabled.contains(&m.id), None)
            } else {
                continue;
            };
            let history = if persistent.contains(&m.id) {
                "kept"
            } else {
                match verdicts.get(&m.id) {
                    Some(Verdict::Kept) => "kept",
                    Some(Verdict::NoBenefit) => "no_benefit",
                    Some(Verdict::Harmful) => "harmful",
                    _ => "untested",
                }
            };
            let s = |v: &dyn erased::Ser| v.str();
            items.push(PlanItem {
                tweak_id: m.id.clone(),
                scope: s(&m.scope),
                risk: s(&m.risk),
                evidence: s(&m.evidence),
                requires_reboot: m.requires_reboot,
                enabled,
                lock,
                history,
                technical: m.notes.clone(),
                before: format!("tw.{}.before", m.id),
                after: format!("tw.{}.after", m.id),
            });
        }
        Ok(PlanPreview {
            game_id: game_id.into(),
            items,
            hidden_in_easy: hidden,
        })
    }

    // ------------------------------------------------------------------ snapshot

    fn hardware_view(&self) -> Option<HardwareSummary> {
        let p = self.profile.as_ref()?;
        let main = p.dedicated_gpu().or(p.gpus.first());
        let secondary = match (p.dedicated_gpu(), p.integrated_gpu()) {
            (Some(_), Some(i)) => Some(FieldView { value: Some(i.name.clone()), confidence: "exact" }),
            _ => None,
        };
        let vram = main.map(|g| g.vram_mb.clone());
        let drive = p.system_drive();
        let ram_speed = match (p.memory.configured_mts.val(), p.memory.rated_mts.val()) {
            (Some(c), Some(r)) => FieldView {
                value: Some(RamSpeed { configured: c, rated: r }),
                confidence: conf(p.memory.rated_mts.confidence.max(p.memory.configured_mts.confidence)),
            },
            _ => FieldView { value: None, confidence: "unknown" },
        };
        Some(HardwareSummary {
            tier: match p.tier {
                Tier::Tier1 => 1,
                Tier::Tier2 => 2,
                Tier::Tier3 => 3,
            },
            is_laptop: p.is_laptop(),
            power_source: match p.power.power_source.val() {
                Some(PowerSource::Battery) => "battery",
                _ => "ac",
            },
            os: p.windows.name.value.clone().unwrap_or_else(|| "Windows".into()),
            cpu: fv(&p.cpu.name),
            gpu: FieldView {
                value: main.map(|g| g.name.clone()),
                confidence: if main.is_some() { "exact" } else { "unknown" },
            },
            gpu_secondary: secondary,
            vram_gb: match vram {
                Some(f) => FieldView {
                    value: f.value.map(|mb| r1(mb as f64 / 1024.0)),
                    confidence: if f.value.is_some() { conf(f.confidence) } else { "unknown" },
                },
                None => FieldView { value: None, confidence: "unknown" },
            },
            ram_gb: FieldView {
                value: p.memory.total_gb.value.map(r1),
                confidence: conf(p.memory.total_gb.confidence),
            },
            ram_speed,
            ram_channels: fv(&p.memory.channels),
            storage: FieldView {
                value: drive.map(|d| {
                    let media = match d.media.val() {
                        Some(MediaType::NvmeSsd) => "NVMe SSD",
                        Some(MediaType::SataSsd) => "SATA SSD",
                        Some(MediaType::Hdd) => "HDD",
                        _ => "Disco",
                    };
                    format!("{media} · {}", d.model.trim())
                }),
                confidence: if drive.is_some() { "exact" } else { "unknown" },
            },
            storage_free_pct: FieldView {
                value: drive
                    .and_then(|d| d.volumes.iter().find(|v| v.letter.starts_with('C')).or(d.volumes.first()))
                    .map(|v| r1(v.free_pct)),
                confidence: if drive.is_some() { "exact" } else { "unknown" },
            },
            board_vendor: fv(&p.firmware.board_manufacturer),
        })
    }

    fn measured(&self, game_id: &str, kept: &[String]) -> Option<Measured> {
        if kept.is_empty() {
            return None;
        }
        let sessions = self.journal.sessions(game_id).ok()?;
        let latest = sessions.iter().rev().find(|s| s.valid)?.context.clone();
        let low1 = |set: &[String]| -> Vec<f64> {
            let key = sorted(set);
            sessions
                .iter()
                .filter(|s| s.valid && s.context == latest && sorted(&s.tweak_set) == key)
                .map(|s| s.metrics.low_1)
                .collect()
        };
        let (base, cand) = (low1(&[]), low1(kept));
        if base.len() < 2 || cand.len() < 2 {
            return None;
        }
        let c = experiment::compare(&base, &cand, true, &self.settings);
        Some(Measured {
            low1_delta_pct: r1(c.delta_pct),
            ci_low: r1(c.ci_low),
            ci_high: r1(c.ci_high),
            runs: c.runs_candidate,
        })
    }

    fn game_card(&self, g: &LibraryGame) -> GameCard {
        let target = self.target(g);
        let (anti_cheat, anti_cheat_name) = match target.anticheat {
            AntiCheat::None => ("none", None),
            AntiCheat::Kernel(k) => ("kernel", Some(ac_name(k).to_string())),
            AntiCheat::UserMode(k) => ("user_mode", Some(ac_name(k).to_string())),
            AntiCheat::Unknown => ("kernel", None),
        };
        let sessions = self.journal.sessions(&g.id).unwrap_or_default();
        let bottleneck = sessions
            .iter()
            .rev()
            .find(|s| s.valid)
            .and_then(|s| s.bottleneck.clone())
            .unwrap_or_else(|| "unknown".into());
        let verdicts = self.orch(&g.id).verdicts(&g.id).unwrap_or_default();
        let mut kept: Vec<String> = verdicts
            .iter()
            .filter(|(_, v)| **v == Verdict::Kept)
            .map(|(k, _)| k.clone())
            .collect();
        kept.sort();
        let status = if self.live.as_ref().is_some_and(|l| l.game_id == g.id) {
            "running"
        } else if self.baseline_runs(&g.id) < self.settings.min_runs {
            "needs_baseline"
        } else if self.armed.contains_key(&g.id) {
            "armed"
        } else if !kept.is_empty() {
            "optimized"
        } else {
            "ready"
        };
        let fixes_ready = self
            .plan_preview(&g.id)
            .map(|p| p.items.iter().filter(|i| i.enabled).count())
            .unwrap_or(0);
        GameCard {
            id: g.id.clone(),
            name: g.name.clone(),
            launcher: g.launcher,
            anti_cheat,
            anti_cheat_name,
            bottleneck,
            bottleneck_params: None,
            status,
            fixes_ready,
            measured: self.measured(&g.id, &kept),
            hue: hue(&g.id),
        }
    }

    fn persistent_view(&self) -> Vec<PersistentChangeView> {
        let mut out: Vec<PersistentChangeView> = Vec::new();
        for t in self
            .journal
            .txs_in(&[TxState::Applied])
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.kind == TxKind::Persistent)
        {
            let drift = t.reason.as_deref().is_some_and(|r| r.contains("driftRepair"));
            for s in self.journal.steps(t.tx_id).unwrap_or_default() {
                if s.state == StepState::Verified && !out.iter().any(|c| c.tweak_id == s.tweak_id) {
                    out.push(PersistentChangeView {
                        tweak_id: s.tweak_id,
                        since: t.created_at,
                        source: if drift { "drift_repair" } else { "user_accept" }.into(),
                    });
                }
            }
        }
        out.extend(self.store.bios_changes.iter().cloned());
        out
    }

    fn suspended_view(&self) -> Vec<SuspendedView> {
        let mut out: Vec<SuspendedView> = Vec::new();
        for t in self
            .journal
            .txs_in(&[TxState::Applied])
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.kind == TxKind::Session)
        {
            for s in self.journal.steps(t.tx_id).unwrap_or_default() {
                if s.state != StepState::Verified {
                    continue;
                }
                let action = match s.action.kind() {
                    "process_suspend" => "suspend",
                    "process_eco_qos" => "eco_qos",
                    "process_priority" => "lower_priority",
                    _ => continue,
                };
                if let Some(p) = s.action.process() {
                    if !out.iter().any(|v| v.image == p.image && v.action == action) {
                        out.push(SuspendedView { image: p.image.clone(), action });
                    }
                }
            }
        }
        out
    }

    fn pulse(&self) -> PulseState {
        if let Some(l) = &self.live {
            return PulseState::Session { label: l.view.bottleneck.clone() };
        }
        match self.issues.iter().find(|i| i.impact != Impact::Low) {
            Some(i) => PulseState::Bottleneck {
                label: if matches!(i.id.as_str(), "game_on_igpu" | "uma_small" | "gpu_pcie_link") {
                    "gpu_bound"
                } else {
                    "memory_bound"
                }
                .into(),
                intensity: if i.impact == Impact::High { 0.8 } else { 0.45 },
            },
            None => PulseState::Idle,
        }
    }

    fn health(&self, persistent: usize) -> HealthSummary {
        let month = month_of(self.platform.now_ms());
        let sessions: usize = self
            .library
            .iter()
            .map(|g| {
                self.journal
                    .sessions(&g.id)
                    .unwrap_or_default()
                    .iter()
                    .filter(|s| s.valid)
                    .count()
            })
            .sum();
        let rec = match self.issues.first().map(|i| i.id.as_str()) {
            Some(id @ ("game_on_igpu" | "ram_below_rated")) => format!("health.rec.{id}"),
            _ => "health.rec.none".into(),
        };
        HealthSummary {
            drift_repaired: self.store.drift_repaired.get(&month).copied().unwrap_or(0),
            month,
            kept_fixed: persistent,
            sessions_measured: sessions,
            recommendation_key: rec,
        }
    }

    fn coach_view(&self) -> Option<CoachState> {
        let c = self.store.coach.as_ref()?;
        Some(CoachState {
            flow_id: c.flow.flow_id.key().into(),
            state: c.state.clone(),
            steps: c
                .flow
                .steps
                .iter()
                .map(|s| CoachStepView {
                    kind: serde_json::to_value(s.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default(),
                    key: s.key.clone(),
                    params: (!s.params.is_empty()).then(|| s.params.clone()),
                })
                .collect(),
            watch_until: c.watch_until,
        })
    }

    pub fn snapshot(&mut self) -> EngineSnapshot {
        if self.games_cache.is_none() {
            let cards = self.library.iter().map(|g| self.game_card(g)).collect();
            self.games_cache = Some(cards);
        }
        let persistent = self.persistent_view();
        EngineSnapshot {
            seq: self.seq,
            boot_id: self.journal.boot_id().to_string(),
            settings: self.store.settings.clone(),
            hardware: self.hardware_view(),
            issues: self
                .issues
                .iter()
                .filter_map(|i| serde_json::to_value(i).ok())
                .collect(),
            pulse: self.pulse(),
            games: self.games_cache.clone().unwrap_or_default(),
            active_session: self.live.as_ref().map(|l| l.view.clone()),
            operations: self.ops.clone(),
            health: Some(self.health(persistent.len())),
            persistent_changes: persistent,
            suspended: self.suspended_view(),
            prompts: self.store.prompts.clone(),
            reports: self.store.reports.clone(),
            coach: self.coach_view(),
        }
    }

    /// Rebuild the snapshot and push what changed.
    fn refresh(&mut self) {
        let snap = self.snapshot();
        let Ok(Value::Object(mut now)) = serde_json::to_value(&snap) else { return };
        now.remove("seq");
        let mut patch = Map::new();
        for (k, v) in &now {
            if self.last.get(k) != Some(v) {
                patch.insert(k.clone(), v.clone());
            }
        }
        self.last = now;
        if !patch.is_empty() {
            self.event("state.patch", Value::Object(patch));
        }
    }

    fn event(&mut self, ty: &str, data: Value) {
        self.seq += 1;
        let f = json!({ "kind": "evt", "seq": self.seq, "type": ty, "data": data });
        self.out.push(Out::Broadcast(f.to_string()));
    }

    pub fn take_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    // ------------------------------------------------------------------ requests

    /// One raw frame from a client. Always answered (malformed input gets an error).
    pub fn request(&mut self, conn: ConnId, raw: &str) {
        let req: Req = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(_) => {
                // No id we can trust: answer on id 0 so the client sees something.
                let f = json!({ "kind": "res", "id": 0, "ok": false, "error": bad("BAD_REQUEST", "err.internal") });
                self.out.push(Out::Reply(conn, f.to_string()));
                return;
            }
        };
        let res = if req.kind != "req" || !METHODS.contains(&req.method.as_str()) {
            Err(bad("UNKNOWN_METHOD", "err.internal"))
        } else {
            self.dispatch(&req.method, &req.params)
        };
        let f = match res {
            Ok(v) => json!({ "kind": "res", "id": req.id, "ok": true, "result": v }),
            Err(e) => json!({ "kind": "res", "id": req.id, "ok": false, "error": e }),
        };
        self.out.push(Out::Reply(conn, f.to_string()));
        if req.method != "state.snapshot" && req.method != "hello" {
            self.games_cache = None;
            self.refresh();
        }
    }

    fn str_param<'v>(p: &'v Value, k: &str, max: usize) -> Result<&'v str, Fail> {
        p.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= max)
            .ok_or_else(|| bad("BAD_PARAMS", "err.internal").with("param", k))
    }

    fn game_param(&self, p: &Value) -> Result<String, Fail> {
        let id = Self::str_param(p, "gameId", 128)?;
        self.game(id)
            .map(|g| g.id.clone())
            .ok_or_else(|| bad("UNKNOWN_GAME", "err.unknown_game"))
    }

    fn tweak_param(&self, p: &Value) -> Result<String, Fail> {
        let id = Self::str_param(p, "tweakId", 64)?;
        if self.catalog.get(id).is_some() || id.starts_with("bios_") {
            Ok(id.into())
        } else {
            Err(bad("UNKNOWN_TWEAK", "err.unknown_tweak"))
        }
    }

    fn dispatch(&mut self, method: &str, p: &Value) -> Result<Value, Fail> {
        let v = |x: &dyn erased::Json| x.json();
        match method {
            "hello" => Ok(json!({
                "bootId": self.journal.boot_id(),
                "svcVersion": self.cfg.svc_version,
                "protocol": 1,
            })),
            "state.snapshot" => {
                self.refresh(); // flush pending changes first: the snapshot is current
                Ok(v(&self.snapshot()))
            }
            "hardware.rescan" => Ok(self.enqueue("hardware.rescan", "op.rescan", None, 2, false, Job::Rescan)),
            "plan.preview" => {
                let g = self.game_param(p)?;
                Ok(v(&self.plan_preview(&g)?))
            }
            "plan.setToggle" => {
                let g = self.game_param(p)?;
                let t = self.tweak_param(p)?;
                let on = p
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| bad("BAD_PARAMS", "err.internal"))?;
                let plan = self.plan_preview(&g)?;
                let item = plan
                    .items
                    .iter()
                    .find(|i| i.tweak_id == t)
                    .ok_or_else(|| bad("UNKNOWN_TWEAK", "err.unknown_tweak"))?;
                if let Some(lock) = item.lock {
                    return Err(bad("TWEAK_LOCKED", &format!("lock.{lock}")));
                }
                self.store.toggles.entry(g.clone()).or_default().insert(t, on);
                self.armed.remove(&g); // the armed plan no longer matches
                self.save();
                Ok(v(&self.plan_preview(&g)?))
            }
            "session.optimize" => {
                let g = self.game_param(p)?;
                if self.live.is_some() {
                    return Err(bad("GAME_RUNNING", "err.game_running"));
                }
                let name = self.game(&g).map(|x| x.name.clone()).unwrap_or_default();
                Ok(self.enqueue("session.optimize", "op.optimize", Some(json!({ "game": name })), 4, true, Job::Optimize(g)))
            }
            "session.benchmark" => {
                let g = self.game_param(p)?;
                let name = self.game(&g).map(|x| x.name.clone()).unwrap_or_default();
                Ok(self.enqueue("session.benchmark", "op.benchmark", Some(json!({ "game": name })), 1, false, Job::Benchmark(g)))
            }
            "persistent.accept" => {
                let t = self.tweak_param(p)?;
                let game = match p.get("gameId") {
                    Some(_) => self.game_param(p)?,
                    None => self.game_with_kept(&t).ok_or_else(|| bad("NOT_PROVEN", "err.not_proven"))?,
                };
                Ok(self.enqueue("persistent.accept", "op.persist", Some(json!({ "tweak": t })), 3, false, Job::Persist { tweak: t, game }))
            }
            "persistent.revert" => {
                let t = self.tweak_param(p)?;
                Ok(self.enqueue("persistent.revert", "op.revert_one", Some(json!({ "tweak": t })), 2, false, Job::RevertOne(t)))
            }
            "tx.revertAll" => {
                if self.live.is_some() {
                    return Err(bad("GAME_RUNNING", "err.game_running"));
                }
                Ok(self.enqueue("tx.revertAll", "op.revert_all", None, 3, true, Job::RevertAll))
            }
            "background.resumeAll" => {
                let n = Engine::new(&self.journal, self.sys.as_ref())
                    .release_processes(None)
                    .map_err(tx_fail)?;
                Ok(json!({ "ok": true, "released": n }))
            }
            "background.setWhitelist" => {
                let list = p
                    .get("images")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() <= 200)
                    .ok_or_else(|| bad("BAD_PARAMS", "err.internal"))?;
                let mut set = std::collections::BTreeSet::new();
                for i in list {
                    let s = i
                        .as_str()
                        .filter(|s| s.len() <= 64 && s.to_ascii_lowercase().ends_with(".exe") && !s.contains(['\\', '/']))
                        .ok_or_else(|| bad("BAD_PARAMS", "err.internal"))?;
                    set.insert(s.to_ascii_lowercase());
                }
                self.store.whitelist = set;
                self.save();
                Ok(json!({ "ok": true }))
            }
            "bios.startFlow" => {
                let id = Self::str_param(p, "issueId", 64)?;
                let issue = self
                    .issues
                    .iter()
                    .find(|i| i.id == id && matches!(i.fixable_by, FixPath::Bios { .. }))
                    .ok_or_else(|| bad("NOT_FOUND", "err.not_found"))?;
                let hw = self.profile.as_ref().ok_or_else(|| bad("NOT_FOUND", "err.not_found"))?;
                let flow = coach::flow_for(issue, hw).ok_or_else(|| bad("NOT_FOUND", "err.not_found"))?;
                self.store.coach = Some(CoachRecord {
                    flow,
                    state: "started".into(),
                    pending_verify: false,
                    watch_from: None,
                    watch_until: None,
                });
                self.save();
                Ok(v(&self.coach_view()))
            }
            "bios.rebootToFirmware" => {
                if p.get("confirm") != Some(&Value::Bool(true)) {
                    return Err(bad("CONFIRM_REQUIRED", "err.confirm_required"));
                }
                if self.store.coach.is_none() {
                    return Err(bad("NOT_FOUND", "err.not_found"));
                }
                if !self.cfg.allow_reboot {
                    return Err(bad("NOT_AVAILABLE", "err.not_available_dev"));
                }
                if self.live.is_some() {
                    return Err(bad("GAME_RUNNING", "err.game_running"));
                }
                Ok(self.enqueue("bios.rebootToFirmware", "op.reboot_fw", None, 2, true, Job::RebootFw))
            }
            "report.session" => {
                let tx = p
                    .get("txId")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| bad("BAD_PARAMS", "err.internal"))?;
                self.reports
                    .get(tx)
                    .map(|r| v(&r))
                    .ok_or_else(|| bad("NOT_FOUND", "err.not_found"))
            }
            "report.health" => {
                let n = self.persistent_view().len();
                Ok(v(&self.health(n)))
            }
            "prompt.answer" => {
                let id = Self::str_param(p, "promptId", 64)?.to_string();
                let choice = p.get("choice").and_then(Value::as_str).unwrap_or("ok").to_string();
                let idx = self
                    .store
                    .prompts
                    .iter()
                    .position(|x| x.id() == id)
                    .ok_or_else(|| bad("NOT_FOUND", "err.not_found"))?;
                let prompt = self.store.prompts.remove(idx);
                if let Prompt::KeepPersistent { tweak_id, game_id, .. } = &prompt {
                    if choice == "yes" {
                        self.enqueue("persistent.accept", "op.persist", Some(json!({ "tweak": tweak_id })), 3, false,
                            Job::Persist { tweak: tweak_id.clone(), game: game_id.clone() });
                    }
                }
                self.save();
                Ok(json!({ "ok": true }))
            }
            "settings.get" => Ok(v(&self.store.settings)),
            "settings.set" => {
                let obj = p.as_object().ok_or_else(|| bad("BAD_PARAMS", "err.internal"))?;
                let mut s = serde_json::to_value(&self.store.settings).unwrap_or(Value::Null);
                for (k, val) in obj {
                    s[k] = val.clone();
                }
                let new: Settings = serde_json::from_value(s).map_err(|_| bad("BAD_PARAMS", "err.internal"))?;
                let ok = ["easy", "expert"].contains(&new.mode.as_str())
                    && ["es", "en"].contains(&new.locale.as_str())
                    && ["auto", "dark", "light"].contains(&new.theme.as_str())
                    && ["auto", "on", "off"].contains(&new.reduce_motion.as_str());
                if !ok {
                    return Err(bad("BAD_PARAMS", "err.internal"));
                }
                if new.mode != self.store.settings.mode {
                    self.armed.clear(); // the plan depends on the mode
                }
                self.store.settings = new;
                self.save();
                Ok(v(&self.store.settings))
            }
            "op.cancel" => {
                let id = Self::str_param(p, "opId", 64)?;
                if self.ops.iter().any(|o| o.op_id == id) {
                    Err(bad("NOT_CANCELLABLE", "err.not_cancellable"))
                } else {
                    Err(bad("NOT_FOUND", "err.not_found"))
                }
            }
            _ => Err(bad("UNKNOWN_METHOD", "err.internal")),
        }
    }

    fn game_with_kept(&self, tweak: &str) -> Option<String> {
        self.library
            .iter()
            .find(|g| self.orch(&g.id).verdicts(&g.id).unwrap_or_default().get(tweak) == Some(&Verdict::Kept))
            .map(|g| g.id.clone())
    }

    // ------------------------------------------------------------------ operations

    fn enqueue(&mut self, method: &str, label: &str, label_params: Option<Value>, total: usize, blocking: bool, job: Job) -> Value {
        let op_id = self.store.new_id("op");
        self.ops.push(OperationView {
            op_id: op_id.clone(),
            method: method.into(),
            label_key: label.into(),
            label_params: label_params.and_then(params_of),
            step: 0,
            total,
            step_key: "step.working".into(),
            step_params: None,
            started_at: self.platform.now_ms(),
            deadline_ms: 60_000,
            cancellable: false,
            blocking,
            state: "queued",
        });
        self.queue.push_back((op_id.clone(), job));
        json!({ "opId": op_id })
    }

    fn step(&mut self, op: &str, n: usize, key: &str, params: Option<Value>) {
        let now = self.platform.now_ms();
        let mut progress = None;
        if let Some(o) = self.ops.iter_mut().find(|o| o.op_id == op) {
            o.state = "running";
            o.step = n;
            o.step_key = key.into();
            o.step_params = params.and_then(params_of);
            progress = Some(json!({ "opId": op, "step": n, "total": o.total, "elapsedMs": now - o.started_at }));
        }
        self.refresh();
        if let Some(pr) = progress {
            self.event("op.progress", pr);
        }
    }

    fn finish(&mut self, op: &str, res: Result<(), Fail>) {
        let method = self
            .ops
            .iter()
            .find(|o| o.op_id == op)
            .map(|o| o.method.clone())
            .unwrap_or_default();
        self.ops.retain(|o| o.op_id != op);
        self.games_cache = None;
        self.refresh();
        let data = match res {
            Ok(()) => json!({ "opId": op, "method": method, "outcome": "done" }),
            Err(e) => json!({ "opId": op, "method": method, "outcome": "failed", "error": e }),
        };
        self.event("op.finished", data);
    }

    /// Run the queued operations (called right after the replies were sent).
    pub fn run_queued(&mut self) {
        while let Some((op, job)) = self.queue.pop_front() {
            let res = self.run_job(&op, job);
            self.save();
            self.finish(&op, res);
        }
    }

    fn run_job(&mut self, op: &str, job: Job) -> Result<(), Fail> {
        match job {
            Job::Rescan => {
                self.step(op, 1, "step.rescan_hw", None);
                self.rescan();
                self.step(op, 2, "step.rescan_issues", None);
                Ok(())
            }
            Job::Benchmark(g) => {
                self.step(op, 1, "step.working", None);
                self.store.opted_in.insert(g.clone());
                let name = self.game(&g).map(|x| x.name.clone()).unwrap_or_default();
                let left = self.settings.min_runs.saturating_sub(self.baseline_runs(&g));
                self.push_notice("notice.play_to_measure", params_of(json!({ "game": name, "n": left.max(1) })));
                Ok(())
            }
            Job::Optimize(g) => {
                let lg = self.game(&g).cloned().ok_or_else(|| bad("UNKNOWN_GAME", "err.unknown_game"))?;
                let target = self.target(&lg);
                let ac = match target.anticheat {
                    AntiCheat::Kernel(k) | AntiCheat::UserMode(k) => ac_name(k),
                    AntiCheat::Unknown => "?",
                    AntiCheat::None => "—",
                };
                self.step(op, 1, "step.check_anticheat", Some(json!({ "ac": ac })));
                self.store.opted_in.insert(g.clone());
                if target.exe_path.is_empty() || self.baseline_runs(&g) < self.settings.min_runs {
                    // Nothing can be compared yet: the next sessions measure the baseline.
                    let left = self.settings.min_runs.saturating_sub(self.baseline_runs(&g));
                    self.push_notice("notice.play_to_measure", params_of(json!({ "game": lg.name, "n": left.max(1) })));
                    return Ok(());
                }
                let hw = self.profile.clone().ok_or_else(|| bad("INTERNAL", "err.internal"))?;
                self.step(op, 2, "step.journal", None);
                if let Some(old) = self.armed.remove(&g) {
                    if let Some(tx) = old.tx {
                        let _ = Engine::new(&self.journal, self.sys.as_ref()).revert(tx);
                    }
                }
                let armed = self.orch(&g).arm(&target, &hw).map_err(tx_fail)?;
                let n = armed.plan.measured.len() + armed.plan.protective.len();
                self.step(op, 3, "step.arm", Some(json!({ "n": n })));
                self.step(op, 4, "step.verify", None);
                self.armed.insert(g, armed);
                Ok(())
            }
            Job::Persist { tweak, game } => {
                let lg = self.game(&game).cloned().ok_or_else(|| bad("UNKNOWN_GAME", "err.unknown_game"))?;
                let hw = self.profile.clone().ok_or_else(|| bad("INTERNAL", "err.internal"))?;
                self.step(op, 1, "step.journal", None);
                let target = self.target(&lg);
                self.step(op, 2, "step.apply", None);
                self.orch(&game).accept_persistent(&target, &hw, &tweak).map_err(tx_fail)?;
                self.step(op, 3, "step.verify", None);
                Ok(())
            }
            Job::RevertOne(tweak) => {
                self.step(op, 1, "step.revert", None);
                if tweak.starts_with("bios_") {
                    // A BIOS setting can only be undone in the BIOS: forget it here.
                    self.store.bios_changes.retain(|c| c.tweak_id != tweak);
                    return Ok(());
                }
                let txs: Vec<i64> = self
                    .journal
                    .txs_in(&[TxState::Applied])
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| t.kind == TxKind::Persistent)
                    .filter(|t| {
                        self.journal
                            .steps(t.tx_id)
                            .unwrap_or_default()
                            .iter()
                            .any(|s| s.tweak_id == tweak && s.state == StepState::Verified)
                    })
                    .map(|t| t.tx_id)
                    .collect();
                if txs.is_empty() {
                    return Err(bad("NOT_FOUND", "err.not_found"));
                }
                let mut failed = Vec::new();
                {
                    let engine = Engine::new(&self.journal, self.sys.as_ref());
                    for tx in txs {
                        let r = engine.revert(tx).map_err(tx_fail)?;
                        failed.extend(r.failed.into_iter().map(|(t, _)| t));
                    }
                }
                self.step(op, 2, "step.verify_revert", None);
                if let Some(t) = failed.first() {
                    return Err(bad("REVERT_FAILED", "err.step_failed").with("step", t.clone()));
                }
                Ok(())
            }
            Job::RevertAll => {
                self.step(op, 1, "step.revert", None);
                self.armed.clear();
                let rep = Engine::new(&self.journal, self.sys.as_ref())
                    .revert_all()
                    .map_err(tx_fail)?;
                self.step(op, 2, "step.resume_background", None);
                self.step(op, 3, "step.diff_journal", None);
                if rep.remaining_diffs.is_empty() {
                    self.push_notice("notice.reset_done", None);
                    Ok(())
                } else {
                    self.push_notice("notice.reset_diffs", params_of(json!({ "n": rep.remaining_diffs.len() })));
                    Err(bad("RESET_INCOMPLETE", "err.step_failed").with("step", rep.remaining_diffs.join(", ")))
                }
            }
            Job::RebootFw => {
                self.step(op, 1, "step.save_state", None);
                if let Some(c) = self.store.coach.as_mut() {
                    c.pending_verify = true;
                }
                self.save();
                self.step(op, 2, "step.reboot_fw", None);
                self.platform.reboot_to_firmware().map_err(|e| {
                    if let Some(c) = self.store.coach.as_mut() {
                        c.pending_verify = false;
                    }
                    bad("REBOOT_FAILED", "err.step_failed").with("step", e)
                })
            }
        }
    }

    // ------------------------------------------------------------------ timers & games

    /// Called by the service loop whenever it is idle (and at least every 100 ms).
    pub fn tick(&mut self) {
        let now = self.platform.now_ms();
        let mut changed = false;
        if now - self.t_user >= USER_MS {
            self.t_user = now;
            let u = self.platform.user_root();
            if u != self.user_root && self.live.is_none() {
                self.user_root = u;
                self.armed.clear();
                self.rescan();
                changed = true;
            }
        }
        if now - self.t_library >= LIBRARY_MS && self.live.is_none() {
            self.t_library = now;
            self.library = self.platform.library(&self.user_root);
            self.games_cache = None;
            changed = true;
        }
        if now - self.t_watch >= WATCH_MS {
            self.t_watch = now;
            changed |= self.watch();
        }
        if self.live.is_some() && now - self.t_hud >= HUD_MS {
            self.t_hud = now;
            self.hud();
            changed = true;
        }
        if now - self.t_drift >= DRIFT_MS && self.live.is_none() {
            self.t_drift = now;
            self.drift(Trigger::Daily);
            self.coach_watch();
            self.save();
            changed = true;
        }
        if now - self.t_refresh >= REFRESH_MS {
            self.t_refresh = now;
            self.games_cache = None;
            changed = true;
        }
        if changed {
            self.refresh();
        }
    }

    /// Detect a game starting or exiting. Returns true if something changed.
    fn watch(&mut self) -> bool {
        let procs = self.platform.processes();
        if let Some(l) = &self.live {
            let alive = procs
                .iter()
                .any(|p| p.process.pid == l.proc.pid && p.process.start_time == l.proc.start_time);
            if !alive {
                self.end_session();
                return true;
            }
            return false;
        }
        let present: HashSet<(u32, u64)> = procs.iter().map(|p| (p.process.pid, p.process.start_time)).collect();
        self.seen.retain(|k, _| present.contains(k));
        for p in &procs {
            if p.session_id == 0 || !p.has_visible_window || is_helper_exe(&p.process.image) {
                continue;
            }
            let key = (p.process.pid, p.process.start_time);
            let hit = match self.seen.get(&key) {
                Some(v) => v.clone(),
                None => {
                    let g = self.platform.image_path(p.process.pid).and_then(|path| {
                        let n = norm(&path);
                        self.library
                            .iter()
                            .find(|g| self.store.learned_exe.get(&g.id).is_some_and(|e| norm(e) == n))
                            .or_else(|| self.library.iter().find(|g| under(&path, &g.install_dir)))
                            .map(|g| (g.id.clone(), path))
                    });
                    if let Some((id, path)) = &g {
                        self.store.learned_exe.insert(id.clone(), path.clone());
                    }
                    let id = g.map(|(id, _)| id);
                    self.seen.insert(key, id.clone());
                    id
                }
            };
            if let Some(game_id) = hit {
                self.start_session(&game_id, p.clone(), &procs);
                return true;
            }
        }
        false
    }

    fn start_session(&mut self, game_id: &str, p: ProcInfo, procs: &[ProcInfo]) {
        let Some(lg) = self.game(game_id).cloned() else { return };
        self.save(); // learned exe
        let target = self.target(&lg);
        let now = self.platform.now_ms();
        let children = overdrive_win::processes::descendants(procs, p.process.pid);
        let armed = self.armed.remove(game_id);
        let active = match (&self.profile, self.store.opted_in.contains(game_id)) {
            (Some(hw), true) => {
                let hw = hw.clone();
                match self.orch(game_id).on_game_start(&target, &hw, armed, &p.process, procs, children) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        self.note(format!("on_game_start failed: {e}"));
                        self.push_notice("notice.apply_failed", params_of(json!({ "game": lg.name })));
                        None
                    }
                }
            }
            _ => None, // not opted in: observe only (measures the baseline, changes nothing)
        };
        let (capture, capture_error) = match self.platform.capture(p.process.pid) {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e)),
        };
        let bottleneck = self
            .games_cache
            .as_ref()
            .and_then(|c| c.iter().find(|g| g.id == game_id))
            .map(|g| g.bottleneck.clone())
            .unwrap_or_else(|| "unknown".into());
        let tweaks_active = active.as_ref().map(|a| a.tweak_set.len()).unwrap_or(0);
        self.live = Some(Live {
            game_id: game_id.into(),
            target,
            proc: p.process.clone(),
            active,
            capture,
            capture_error,
            frames: Vec::new(),
            view: SessionView {
                game_id: game_id.into(),
                started_at: now,
                fps: 0.0,
                low1: 0.0,
                frametime_ms: 0.0,
                gpu_busy_ms: 0.0,
                recent: vec![],
                bottleneck,
                tweaks_active,
            },
        });
        self.games_cache = None;
    }

    fn hud(&mut self) {
        let Some(l) = self.live.as_mut() else { return };
        if let Some(c) = l.capture.as_mut() {
            l.frames.extend(c.poll());
        }
        let Some(last) = l.frames.last().copied() else { return };
        let n = l.frames.len();
        let recent: Vec<f64> = l.frames[n.saturating_sub(120)..].iter().map(|f| f.frametime_ms).collect();
        let window: Vec<f64> = l
            .frames
            .iter()
            .rev()
            .take_while(|f| f.t_ms >= last.t_ms - 1000.0)
            .map(|f| f.frametime_ms)
            .collect();
        let mut lowwin: Vec<f64> = l.frames[n.saturating_sub(1200)..].iter().map(|f| f.frametime_ms).collect();
        lowwin.sort_by(f64::total_cmp);
        let mean = window.iter().sum::<f64>() / window.len().max(1) as f64;
        l.view.fps = r1(if mean > 0.0 { 1000.0 / mean } else { 0.0 });
        let p99 = percentile(&lowwin, 99.0);
        l.view.low1 = r1(if p99 > 0.0 { 1000.0 / p99 } else { 0.0 });
        l.view.frametime_ms = (last.frametime_ms * 100.0).round() / 100.0;
        l.view.gpu_busy_ms = r1(last.gpu_busy_ms.unwrap_or(0.0));
        l.view.recent = recent.iter().map(|x| (x * 100.0).round() / 100.0).collect();
    }

    fn end_session(&mut self) {
        let Some(mut l) = self.live.take() else { return };
        if let Some(c) = l.capture.as_mut() {
            l.frames.extend(c.poll());
        }
        let samples = l.capture.take().map(|c| c.finish()).unwrap_or_default();
        let on_battery = samples.iter().rev().find_map(|s| s.on_battery).unwrap_or(false);
        let driver = self
            .profile
            .as_ref()
            .and_then(|p| p.dedicated_gpu().or(p.gpus.first()))
            .and_then(|g| g.driver_version.value.clone());
        // Only what must match for two sessions to be comparable.
        let ctx = json!({ "driverVersion": driver, "powerSource": if on_battery { "battery" } else { "ac" } });
        let hw = self.profile.clone();
        let name = self.game(&l.game_id).map(|g| g.name.clone()).unwrap_or_default();
        let now = self.platform.now_ms();
        let series = downsample(&l.frames.iter().map(|f| f.frametime_ms).collect::<Vec<_>>(), 240);

        let (session_id, valid, metrics, label, report) = match (l.active.take(), &hw) {
            (Some(active), Some(hw)) => {
                match self.orch(&l.game_id).on_game_exit(&l.target, hw, active, &l.frames, &samples, &ctx) {
                    Ok(r) => {
                        let label = serde_json::to_value(r.bottleneck).ok().and_then(|v| v.as_str().map(String::from));
                        (r.session_id, r.valid, r.metrics, label, Some(r))
                    }
                    Err(e) => {
                        self.note(format!("on_game_exit failed: {e}"));
                        self.push_notice("notice.revert_failed", params_of(json!({ "game": name })));
                        return;
                    }
                }
            }
            _ => {
                // Observe-only: record what really ran (accepted persistent tweaks are in effect).
                let cls = classify(&l.frames, &samples, &Thresholds::default());
                let metrics = compute(&l.frames, &cls.non_gameplay);
                let valid = metrics.gameplay_secs >= self.settings.min_gameplay_secs;
                let label = serde_json::to_value(cls.label).ok().and_then(|v| v.as_str().map(String::from));
                let set = self.persistent_measured();
                match self.journal.record_session(None, &l.game_id, &set, &ctx, &metrics, label.as_deref(), valid) {
                    Ok(id) => (id, valid, metrics, label, None),
                    Err(e) => {
                        self.note(format!("record_session failed: {e}"));
                        return;
                    }
                }
            }
        };
        let _ = self.reports.put_series(session_id, &series);

        match report {
            Some(r) if !r.tweak_set.is_empty() => {
                if r.reverted.iter().any(|x| !x.ok()) {
                    self.push_notice("notice.revert_failed", params_of(json!({ "game": name })));
                }
                if !valid {
                    self.session_short_notice(&l, &name);
                }
                let ui = self.build_report(&l.game_id, &r, label.unwrap_or_else(|| "unknown".into()), series, now);
                let _ = self.reports.put(&ui);
                self.store.reports.insert(
                    0,
                    ReportSummary {
                        tx_id: ui.tx_id,
                        game_id: l.game_id.clone(),
                        ended_at: now,
                        low1_delta_pct: if ui.baseline.low1 > 0.0 { r1((ui.optimized.low1 / ui.baseline.low1 - 1.0) * 100.0) } else { 0.0 },
                    },
                );
                self.store.reports.truncate(50);
                for t in &r.offers {
                    let c = r.decisions.iter().find(|(d, _)| d == t).map(|(_, c)| *c);
                    let id = self.store.new_id("p");
                    self.store.prompts.push(Prompt::KeepPersistent {
                        prompt_id: id,
                        tweak_id: t.clone(),
                        game_id: l.game_id.clone(),
                        runs: c.as_ref().map(|c| c.runs_candidate).unwrap_or(0),
                        delta_pct: c.map(|c| c.delta_pct).unwrap_or(0.0),
                    });
                }
                self.event("session.report", json!({ "txId": ui.tx_id }));
            }
            _ => {
                let _ = metrics;
                if valid {
                    let left = self.settings.min_runs.saturating_sub(self.baseline_runs(&l.game_id));
                    if left > 0 {
                        self.push_notice("notice.baseline_progress", params_of(json!({ "game": name, "n": left })));
                    } else {
                        self.push_notice("notice.baseline_saved", params_of(json!({ "game": name })));
                    }
                } else {
                    self.session_short_notice(&l, &name);
                }
            }
        }
        self.save();
        self.games_cache = None;
    }

    fn session_short_notice(&mut self, l: &Live, name: &str) {
        if let Some(e) = &l.capture_error {
            self.note(format!("no capture: {e}"));
            self.push_notice("notice.no_capture", params_of(json!({ "game": name })));
        } else {
            let mins = (self.settings.min_gameplay_secs / 60.0).ceil() as u64;
            self.push_notice("notice.session_short", params_of(json!({ "game": name, "min": mins.max(1) })));
        }
    }

    fn build_report(
        &self,
        game_id: &str,
        r: &overdrive_core::session::SessionReport,
        label: String,
        series: Vec<f64>,
        now: i64,
    ) -> SessionReport {
        let sessions = self.journal.sessions(game_id).unwrap_or_default();
        let ctx = sessions.iter().find(|s| s.session_id == r.session_id).map(|s| s.context.clone());
        let base: Vec<_> = sessions
            .iter()
            .filter(|s| s.valid && s.tweak_set.is_empty() && Some(&s.context) == ctx.as_ref())
            .collect();
        let base_metrics: Vec<&SessionMetrics> = base.iter().map(|s| &s.metrics).collect();
        let baseline = mean_metrics(&base_metrics).unwrap_or_else(|| metrics_view(&r.metrics));
        let baseline_series = base
            .last()
            .and_then(|s| self.reports.get_series(s.session_id))
            .unwrap_or_default();
        let outcomes: BTreeMap<String, _> = self
            .journal
            .outcomes(game_id)
            .unwrap_or_default()
            .into_iter()
            .map(|o| (o.tweak_id.clone(), o))
            .collect();
        let tweaks = r
            .tweak_set
            .iter()
            .map(|t| {
                if let Some((_, c)) = r.decisions.iter().find(|(d, _)| d == t) {
                    TweakVerdict {
                        tweak_id: t.clone(),
                        verdict: c.verdict.as_str().into(),
                        delta_pct: (c.verdict != Verdict::NoBenefit).then_some(r1(c.delta_pct)),
                    }
                } else if let Some(o) = outcomes.get(t) {
                    TweakVerdict {
                        tweak_id: t.clone(),
                        verdict: o.verdict.clone(),
                        delta_pct: o.delta_pct.map(r1),
                    }
                } else {
                    TweakVerdict {
                        tweak_id: t.clone(),
                        verdict: "inconclusive".into(),
                        delta_pct: None,
                    }
                }
            })
            .collect();
        SessionReport {
            tx_id: r.session_id,
            game_id: game_id.into(),
            ended_at: now,
            gameplay_secs: r1(r.metrics.gameplay_secs),
            bottleneck: label,
            baseline_runs: base.len(),
            baseline,
            optimized: metrics_view(&r.metrics),
            baseline_series,
            optimized_series: series,
            tweaks,
        }
    }

    /// Service stop: a running game's session and every armed game go back first.
    pub fn shutdown(&mut self) {
        if self.live.is_some() {
            self.end_session();
        }
        {
            let engine = Engine::new(&self.journal, self.sys.as_ref());
            for (_, a) in self.armed.drain() {
                if let Some(tx) = a.tx {
                    let _ = engine.revert(tx);
                }
            }
        }
        self.save();
    }

    // ------------------------------------------------------------------ test hooks

    pub fn live_game(&self) -> Option<&str> {
        self.live.as_ref().map(|l| l.game_id.as_str())
    }
    pub fn journal(&self) -> &Journal {
        &self.journal
    }
}

/// Tiny helpers so `serde` enums turn into their wire strings without boilerplate.
mod erased {
    pub trait Ser {
        fn str(&self) -> String;
    }
    impl<T: serde::Serialize> Ser for T {
        fn str(&self) -> String {
            serde_json::to_value(self)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default()
        }
    }
    pub trait Json {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
        }
    }
}
