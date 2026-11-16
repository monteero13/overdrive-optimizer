//! Phase 6 DoD, end to end on the FakeSystem:
//! catalog → staged A/B → verdicts → auto-revert of useless tweaks → blacklist of harmful
//! ones → promotion of proven wins → drift repair after a simulated Windows update.
//!
//! The simulated game's performance depends ONLY on what is really applied in the fake
//! system during the session (hidden "true effects" + session-to-session noise). The
//! orchestrator has to discover every effect by itself.

use std::collections::{HashMap, HashSet};

use overdrive_core::build_profile;
use overdrive_core::catalog::{
    merge_flag, Build, Catalog, GameTarget, PlanContext, BUILDERS, GPU_PREFS_KEY, HIGH_PERFORMANCE,
    TELEMETRY_TASKS,
};
use overdrive_core::drift::{Cause, DriftGuardian, Finding, RepairOutcome, Trigger};
use overdrive_core::engine::fake::{FakeProc, FakeState, FakeSystem};
use overdrive_core::engine::journal::Journal;
use overdrive_core::engine::tx::Engine;
use overdrive_core::engine::{PriorityClass, ProcRef, RegData, RegRoot, StartType};
use overdrive_core::experiment::Verdict;
use overdrive_core::safety::{AcKind, AntiCheat, ProcInfo, SafetyPolicy};
use overdrive_core::session::{Exclusion, Orchestrator};
use overdrive_core::synth::Rng;
use overdrive_core::telemetry::metrics::SessionMetrics;
use overdrive_core::telemetry::FrameSample;
use overdrive_core::testing::FakeSource;

const HKCU: RegRoot = RegRoot::CurrentUser;
const HKLM: RegRoot = RegRoot::LocalMachine;
const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const EXE: &str = r"C:\Games\Fortnite\FortniteClient-Win64-Shipping.exe";
const EXE_NAME: &str = "FortniteClient-Win64-Shipping.exe";

fn proc_state(pid: u32, image: &str) -> (u32, FakeProc) {
    (
        pid,
        FakeProc {
            start_time: 1000 + pid as u64,
            image: image.into(),
            priority: PriorityClass::Normal,
            eco_qos: false,
            suspended: false,
            alive: true,
        },
    )
}
fn pref(pid: u32, image: &str) -> ProcRef {
    ProcRef {
        pid,
        start_time: 1000 + pid as u64,
        image: image.into(),
    }
}

fn original() -> FakeState {
    let mut s = FakeState {
        power: BALANCED.into(),
        ..Default::default()
    };
    s.power_schemes.insert(HIGH_PERFORMANCE.into());
    s.add_reg(
        &HKCU,
        r"System\GameConfigStore",
        "GameDVR_Enabled",
        RegData::Dword(1),
    );
    s.add_reg(&HKCU, r"Software\Microsoft\Office", "x", RegData::Dword(1));
    s.add_reg(
        &HKLM,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuild",
        RegData::String("26100".into()),
    );
    s.services.insert("DiagTrack".into(), StartType::Automatic);
    s.tasks.insert(TELEMETRY_TASKS[0].into(), true);
    s.tasks.insert(TELEMETRY_TASKS[2].into(), true);
    s.procs.extend([
        proc_state(100, "chrome.exe"),
        proc_state(200, "GoogleUpdate.exe"),
        proc_state(300, EXE_NAME),
        proc_state(400, "Discord.exe"),
    ]);
    s
}

fn procs() -> Vec<ProcInfo> {
    let i = |pid, img: &str, window| ProcInfo {
        process: pref(pid, img),
        session_id: 1,
        parent_pid: None,
        is_foreground: pid == 300,
        produces_audio: pid == 400,
        has_visible_window: window,
    };
    vec![
        i(100, "chrome.exe", true),
        i(200, "GoogleUpdate.exe", false),
        i(300, EXE_NAME, true),
        i(400, "Discord.exe", true),
    ]
}

fn game() -> GameTarget {
    GameTarget {
        id: "epic:fortnite".into(),
        exe_path: EXE.into(),
        exe_name: EXE_NAME.into(),
        anticheat: AntiCheat::Kernel(AcKind::EasyAntiCheat),
        install_dir: String::new(),
    }
}

fn reg(s: &FakeState, root: &RegRoot, key: &str, name: &str) -> Option<RegData> {
    s.reg
        .get(&(
            format!("{root:?}"),
            key.to_ascii_lowercase(),
            name.to_ascii_lowercase(),
        ))
        .cloned()
}

/// What is REALLY in effect right now, read from the fake system.
fn active_effects(s: &FakeState) -> HashSet<&'static str> {
    let mut e = HashSet::new();
    if matches!(reg(s, &HKCU, GPU_PREFS_KEY, EXE), Some(RegData::String(v)) if v.contains("GpuPreference=2"))
    {
        e.insert("hybrid_gpu");
    }
    if s.power == HIGH_PERFORMANCE {
        e.insert("power_session");
    }
    if reg(s, &HKCU, r"System\GameConfigStore", "GameDVR_Enabled") == Some(RegData::Dword(0)) {
        e.insert("gamedvr_off");
    }
    if s.procs[&100].eco_qos {
        e.insert("background_isolation");
    }
    let ifeo = format!(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\{EXE_NAME}\PerfOptions"
    );
    if reg(s, &HKLM, &ifeo, "CpuPriorityClass") == Some(RegData::Dword(3)) {
        e.insert("game_priority");
    }
    if matches!(reg(s, &HKCU, GPU_PREFS_KEY, "DirectXUserGlobalSettings"), Some(RegData::String(v)) if v.contains("SwapEffectUpgradeEnable=1"))
    {
        e.insert("windowed_opt");
    }
    if s.services["DiagTrack"] == StartType::Manual {
        e.insert("background_noise");
    }
    e
}

/// Hidden truth (% change of the 1 % low). The orchestrator never sees this table.
fn true_gain(e: &HashSet<&str>) -> f64 {
    let t: HashMap<&str, f64> = [
        ("hybrid_gpu", 30.0),
        ("power_session", 6.0),
        ("gamedvr_off", 5.0),
        ("background_isolation", -8.0),
        ("game_priority", 0.0),
        ("windowed_opt", 0.0),
        ("background_noise", 0.0),
    ]
    .into();
    e.iter().map(|k| t[k]).sum()
}

fn frames(rng: &mut Rng, gain_pct: f64, stutters_per_min: f64) -> Vec<FrameSample> {
    let session_noise = 1.0 + rng.sym() * 0.035; // ≈ 2 % sd between sessions
    let base = 16.7 / (1.0 + gain_pct / 100.0) * session_noise;
    // Simulate by DURATION (70 s of gameplay), like a real session: faster frames = more frames.
    let n = (70_000.0 / base) as usize;
    let spikes = (stutters_per_min * 70.0 / 60.0).round() as usize;
    let mut spike_at = HashSet::new();
    while spike_at.len() < spikes {
        spike_at.insert(100 + (rng.next_f64() * (n - 200) as f64) as usize);
    }
    let mut t = 0.0;
    (0..n)
        .map(|i| {
            let mut ft = base * (1.0 + rng.sym() * 0.06);
            if spike_at.contains(&i) {
                ft = base * 3.0 + 12.0;
            }
            let f = FrameSample {
                t_ms: t,
                frametime_ms: ft,
                gpu_busy_ms: Some(ft * 0.9),
                display_latency_ms: None,
            };
            t += ft;
            f
        })
        .collect()
}

struct Run {
    verdict_session: HashMap<String, usize>,
    eco_seen_in_session: Vec<bool>,
    sessions: usize,
}

fn play_until_converged(
    orch: &Orchestrator,
    fake: &FakeSystem,
    hw: &overdrive_core::HardwareProfile,
    max: usize,
) -> Run {
    play_seeded(orch, fake, hw, max, 7)
}

fn play_seeded(
    orch: &Orchestrator,
    fake: &FakeSystem,
    hw: &overdrive_core::HardwareProfile,
    max: usize,
    seed: u64,
) -> Run {
    let g = game();
    let mut rng = Rng::new(seed);
    let mut run = Run {
        verdict_session: HashMap::new(),
        eco_seen_in_session: vec![],
        sessions: 0,
    };
    let ctx = serde_json::json!({ "driverVersion": "32.0.15.6094", "settingsHash": "abc", "powerSource": "ac" });
    for i in 0..max {
        let armed = orch.arm(&g, hw).unwrap();
        let active = orch
            .on_game_start(
                &g,
                hw,
                Some(armed),
                &pref(300, EXE_NAME),
                &procs(),
                HashSet::new(),
            )
            .unwrap();
        let state = fake.snapshot();
        let eff = active_effects(&state);
        run.eco_seen_in_session
            .push(eff.contains("background_isolation"));
        assert!(
            state.procs[&400].priority == PriorityClass::Normal && !state.procs[&400].eco_qos,
            "Discord (voice, audio) never touched"
        );
        let stutters = if eff.contains("background_noise") {
            1.5
        } else {
            6.0
        };
        let f = frames(&mut rng, true_gain(&eff), stutters);
        let report = orch.on_game_exit(&g, hw, active, &f, &[], &ctx).unwrap();
        assert!(
            report.reverted.iter().all(|r| r.ok()),
            "session {i}: {:?}",
            report.reverted
        );
        let idle = fake.snapshot();
        assert!(
            !idle.procs[&200].suspended && !idle.procs[&100].eco_qos,
            "background apps restored after every session"
        );
        assert_eq!(
            idle.power, BALANCED,
            "power plan restored after every session"
        );
        for (t, _) in &report.decisions {
            run.verdict_session.insert(t.clone(), i);
        }
        for offer in &report.offers {
            orch.accept_persistent(&g, hw, offer).unwrap(); // the user says "yes, keep it"
        }
        run.sessions = i + 1;
        if report.converged {
            break;
        }
    }
    run
}

#[test]
fn catalog_and_builders_match() {
    let c = Catalog::builtin();
    let ids: HashSet<&str> = c.tweaks.iter().map(|t| t.id.as_str()).collect();
    let builders: HashSet<&str> = BUILDERS.iter().map(|(b, _)| *b).collect();
    assert_eq!(
        ids, builders,
        "every catalog entry needs a builder and vice versa"
    );
    let orders: HashSet<u32> = c.tweaks.iter().map(|t| t.order).collect();
    assert_eq!(orders.len(), c.tweaks.len(), "unique experiment order");
    assert!(
        c.tweaks.iter().all(|t| !t.notes.is_empty()),
        "every entry explains itself"
    );
    assert!(
        Catalog::parse("[[tweak]]\nid = \"x\"\nbogus = 1").is_err(),
        "unknown fields are rejected"
    );
}

#[test]
fn merge_flag_keeps_other_flags() {
    assert_eq!(merge_flag("", "GpuPreference", "2"), "GpuPreference=2;");
    assert_eq!(
        merge_flag(
            "SwapEffectUpgradeEnable=1;GpuPreference=1;",
            "GpuPreference",
            "2"
        ),
        "SwapEffectUpgradeEnable=1;GpuPreference=2;"
    );
    assert_eq!(
        merge_flag("VRROptimizeEnable=0;", "SwapEffectUpgradeEnable", "1"),
        "VRROptimizeEnable=0;SwapEffectUpgradeEnable=1;"
    );
}

#[test]
fn builders_read_state_and_respect_conditions() {
    let c = Catalog::builtin();
    let fake = FakeSystem::new(original());
    let hw = build_profile(&FakeSource::laptop_hybrid());
    let g = game();
    let policy = SafetyPolicy::default();
    let caps: Vec<String> = vec![];
    let dirs = overdrive_core::games::UserDirs::default();
    let ctx = PlanContext {
        game: &g,
        hw: &hw,
        user_root: HKCU,
        procs: None,
        policy: &policy,
        capabilities: &caps,
        now_hour: 20,
        games: &c.games,
        dirs: &dirs,
        baseline: None,
        expert: false,
    };
    assert!(
        matches!(c.build("hybrid_gpu", &ctx, &fake).unwrap(), Build::Actions(ref a) if a.len() == 1)
    );
    assert_eq!(
        c.build("game_mode", &ctx, &fake).unwrap(),
        Build::AlreadyOptimal,
        "Game Mode is on by default"
    );
    assert_eq!(
        c.build("frame_cap", &ctx, &fake).unwrap(),
        Build::NotApplicable("needs_baseline")
    );
    assert_eq!(
        c.build("background_isolation", &ctx, &fake).unwrap(),
        Build::Deferred
    );
    match c.build("background_noise", &ctx, &fake).unwrap() {
        Build::Actions(a) => assert_eq!(
            a.len(),
            3,
            "DiagTrack + the 2 tasks that exist and are enabled: {a:?}"
        ),
        other => panic!("{other:?}"),
    }
    // Desktop without a second GPU → hybrid fix doesn't apply; on battery → no power plan.
    let desktop = build_profile(&FakeSource::desktop_entry());
    let ctx2 = PlanContext {
        hw: &desktop,
        ..ctx
    };
    assert_eq!(
        c.build("hybrid_gpu", &ctx2, &fake).unwrap(),
        Build::NotApplicable("no_hybrid_gpu")
    );
    let mut bat = FakeSource::laptop_hybrid();
    bat.power.ac_online = Some(false);
    let hw_bat = build_profile(&bat);
    let ctx3 = PlanContext {
        game: &g,
        hw: &hw_bat,
        user_root: HKCU,
        procs: None,
        policy: &policy,
        capabilities: &caps,
        now_hour: 20,
        games: &c.games,
        dirs: &dirs,
        baseline: None,
        expert: false,
    };
    assert_eq!(
        c.build("power_session", &ctx3, &fake).unwrap(),
        Build::NotApplicable("on_battery")
    );
}

#[test]
fn full_ab_loop_discovers_every_true_effect() {
    let catalog = Catalog::builtin();
    let fake = FakeSystem::new(original());
    let journal = Journal::open_in_memory("test").unwrap();
    let hw = build_profile(&FakeSource::laptop_hybrid());
    let mut orch = Orchestrator::new(&journal, &fake, &catalog);
    orch.expert = true;

    let (_, excluded) = orch.candidates(&game(), &hw).unwrap();
    let ex: HashMap<String, Exclusion> = excluded.into_iter().collect();
    assert_eq!(ex["game_mode"], Exclusion::AlreadyOptimal);
    assert_eq!(
        ex["frame_cap"],
        Exclusion::NotApplicable("needs_baseline".into())
    );
    assert_eq!(ex["hags"], Exclusion::NeedsReboot);

    let run = play_until_converged(&orch, &fake, &hw, 80);
    let v = orch.verdicts("epic:fortnite").unwrap();
    println!("converged after {} sessions: {v:?}", run.sessions);
    assert_eq!(v.get("hybrid_gpu"), Some(&Verdict::Kept));
    assert_eq!(v.get("power_session"), Some(&Verdict::Kept));
    assert_eq!(v.get("gamedvr_off"), Some(&Verdict::Kept));
    assert_eq!(
        v.get("background_noise"),
        Some(&Verdict::Kept),
        "judged by stutters/min"
    );
    assert_eq!(v.get("background_isolation"), Some(&Verdict::Harmful));
    assert_eq!(v.get("game_priority"), Some(&Verdict::NoBenefit));
    assert_eq!(v.get("windowed_opt"), Some(&Verdict::NoBenefit));
    assert!(run.sessions <= 60, "too slow to converge: {}", run.sessions);

    // Harmful → blacklisted: never applied again after the verdict.
    let k = run.verdict_session["background_isolation"];
    assert!(
        run.eco_seen_in_session[k + 1..].iter().all(|on| !on),
        "harmful tweak applied again after its verdict"
    );
    let (_, excluded) = orch.candidates(&game(), &hw).unwrap();
    assert!(excluded.contains(&("background_isolation".into(), Exclusion::Blacklisted)));

    // Proven persistent wins were offered and accepted; session-only wins are not persistent.
    let persistent = orch.persistent_tweaks().unwrap();
    assert_eq!(
        persistent,
        HashSet::from([
            "hybrid_gpu".to_string(),
            "gamedvr_off".to_string(),
            "background_noise".to_string()
        ])
    );

    // At rest: only the accepted persistent changes remain — no trace of useless tweaks.
    let idle = fake.snapshot();
    let eff = active_effects(&idle);
    assert_eq!(
        eff,
        HashSet::from(["hybrid_gpu", "gamedvr_off", "background_noise"]),
        "{eff:?}"
    );

    // The kernel anti-cheat game itself was never opened, in any session.
    assert!(!fake.opened.borrow().contains(&300));

    // "Put my PC back as it was" → byte-identical to before Overdrive.
    let rep = Engine::new(&journal, &fake).revert_all().unwrap();
    assert!(rep.remaining_diffs.is_empty(), "{rep:?}");
    assert_eq!(fake.snapshot(), original());
}

#[test]
fn easy_mode_only_tests_low_risk_proven_tweaks() {
    let catalog = Catalog::builtin();
    let fake = FakeSystem::new(original());
    let journal = Journal::open_in_memory("test").unwrap();
    let hw = build_profile(&FakeSource::laptop_hybrid());
    let orch = Orchestrator::new(&journal, &fake, &catalog); // expert = false
    let (cands, excluded) = orch.candidates(&game(), &hw).unwrap();
    assert_eq!(
        cands,
        vec![
            "hybrid_gpu",
            "power_session",
            "gamedvr_off",
            "background_noise"
        ]
    );
    let ex: HashMap<String, Exclusion> = excluded.into_iter().collect();
    assert_eq!(ex["background_isolation"], Exclusion::EasyMode);
    assert_eq!(ex["game_priority"], Exclusion::EasyMode);
}

#[test]
fn without_arming_before_launch_tweaks_are_not_counted() {
    let catalog = Catalog::builtin();
    let fake = FakeSystem::new(original());
    let journal = Journal::open_in_memory("test").unwrap();
    let hw = build_profile(&FakeSource::laptop_hybrid());
    let orch = Orchestrator::new(&journal, &fake, &catalog);
    let g = game();
    let mut rng = Rng::new(3);
    let ctx = serde_json::json!({});
    // Baseline sessions, then the plan wants to test hybrid_gpu (before-launch).
    for _ in 0..orch.settings.min_runs {
        let a = orch
            .on_game_start(
                &g,
                &hw,
                Some(orch.arm(&g, &hw).unwrap()),
                &pref(300, EXE_NAME),
                &procs(),
                HashSet::new(),
            )
            .unwrap();
        orch.on_game_exit(&g, &hw, a, &frames(&mut rng, 0.0, 6.0), &[], &ctx)
            .unwrap();
    }
    assert_eq!(
        orch.plan(&g, &hw).unwrap().testing.as_deref(),
        Some("hybrid_gpu")
    );
    // The user launches the game directly, without pressing "Optimize".
    let a = orch
        .on_game_start(
            &g,
            &hw,
            None,
            &pref(300, EXE_NAME),
            &procs(),
            HashSet::new(),
        )
        .unwrap();
    assert!(
        !a.tweak_set.contains(&"hybrid_gpu".to_string()),
        "can't claim an effect that wasn't in place at launch"
    );
    assert_eq!(a.testing, None);
    orch.on_game_exit(&g, &hw, a, &frames(&mut rng, 0.0, 6.0), &[], &ctx)
        .unwrap();
}

#[test]
fn drift_after_windows_update_is_detected_and_repaired() {
    let catalog = Catalog::builtin();
    let fake = FakeSystem::new(original());
    let journal = Journal::open_in_memory("test").unwrap();
    let hw = build_profile(&FakeSource::laptop_hybrid());
    let orch = Orchestrator::new(&journal, &fake, &catalog);
    play_until_converged(&orch, &fake, &hw, 80);
    assert!(orch.persistent_tweaks().unwrap().contains("gamedvr_off"));

    let guardian = DriftGuardian {
        journal: &journal,
        sys: &fake,
        catalog: &catalog,
    };
    assert!(
        guardian.check(Trigger::Boot).unwrap().is_empty(),
        "nothing drifted yet"
    );

    // A Windows feature update turns Game DVR back on.
    fake.state.borrow_mut().add_reg(
        &HKCU,
        r"System\GameConfigStore",
        "GameDVR_Enabled",
        RegData::Dword(1),
    );
    let findings = guardian.check(Trigger::WindowsUpdate).unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        matches!(&findings[0], Finding::Reverted { tweak_id, .. } if tweak_id == "gamedvr_off")
    );
    assert!(
        matches!(
            guardian.repair(&findings[0], false).unwrap(),
            RepairOutcome::Repaired { .. }
        ),
        "low risk + already accepted → repaired automatically"
    );
    assert_eq!(
        reg(
            &fake.snapshot(),
            &HKCU,
            r"System\GameConfigStore",
            "GameDVR_Enabled"
        ),
        Some(RegData::Dword(0))
    );
    assert!(
        guardian.check(Trigger::Daily).unwrap().is_empty(),
        "repaired"
    );

    // And the global reset still gets back to the pre-Overdrive state.
    let rep = Engine::new(&journal, &fake).revert_all().unwrap();
    assert!(rep.remaining_diffs.is_empty(), "{rep:?}");
    assert_eq!(fake.snapshot(), original());
}

#[test]
fn performance_regression_points_at_the_driver_update() {
    let catalog = Catalog::builtin();
    let fake = FakeSystem::new(original());
    let journal = Journal::open_in_memory("test").unwrap();
    let set = vec!["gamedvr_off".to_string(), "hybrid_gpu".to_string()];
    let m = |low_1| SessionMetrics {
        frames: 4000,
        gameplay_secs: 70.0,
        avg_fps: 70.0,
        low_1,
        low_01: 40.0,
        p50_ms: 14.0,
        p95_ms: 16.0,
        p99_ms: 20.0,
        stdev_ms: 1.5,
        stutters: 2,
    };
    for low in [60.0, 61.0, 59.5, 60.5] {
        journal
            .record_session(
                None,
                "g",
                &set,
                &serde_json::json!({ "driverVersion": "560.70", "settingsHash": "a" }),
                &m(low),
                None,
                true,
            )
            .unwrap();
    }
    for low in [50.0, 49.0, 51.0] {
        journal
            .record_session(
                None,
                "g",
                &set,
                &serde_json::json!({ "driverVersion": "565.90", "settingsHash": "a" }),
                &m(low),
                None,
                true,
            )
            .unwrap();
    }
    let guardian = DriftGuardian {
        journal: &journal,
        sys: &fake,
        catalog: &catalog,
    };
    match guardian.regression("g", &set).unwrap() {
        Some(Finding::Regression {
            drop_pct,
            cause: Cause::DriverChanged { from, to },
            ..
        }) => {
            assert!(drop_pct < -10.0);
            assert_eq!(
                (from.as_deref(), to.as_deref()),
                (Some("560.70"), Some("565.90"))
            );
        }
        other => panic!("{other:?}"),
    }
}

/// Statistical robustness: the same hidden truth, 20 different noise seeds.
#[test]
fn verdicts_are_robust_across_noise_seeds() {
    let truth: HashMap<&str, Verdict> = [
        ("hybrid_gpu", Verdict::Kept),
        ("power_session", Verdict::Kept),
        ("gamedvr_off", Verdict::Kept),
        ("background_noise", Verdict::Kept),
        ("background_isolation", Verdict::Harmful),
        ("game_priority", Verdict::NoBenefit),
        ("windowed_opt", Verdict::NoBenefit),
    ]
    .into();
    let (mut correct, mut total, mut wrong_keeps, mut sessions) = (0, 0, 0, Vec::new());
    for seed in 1..=20u64 {
        let catalog = Catalog::builtin();
        let fake = FakeSystem::new(original());
        let journal = Journal::open_in_memory("t").unwrap();
        let hw = build_profile(&FakeSource::laptop_hybrid());
        let mut orch = Orchestrator::new(&journal, &fake, &catalog);
        orch.expert = true;
        let run = play_seeded(&orch, &fake, &hw, 80, seed * 7919);
        sessions.push(run.sessions);
        let v = orch.verdicts("epic:fortnite").unwrap();
        for (t, want) in &truth {
            total += 1;
            if v.get(*t) != Some(want) {
                println!("seed {seed}: {t} → {:?} (want {want:?})", v.get(*t));
            }
            match v.get(*t) {
                Some(got) if got == want => correct += 1,
                Some(Verdict::Kept) => wrong_keeps += 1,
                _ => {}
            }
        }
    }
    let acc = correct as f64 / total as f64 * 100.0;
    sessions.sort();
    println!("robustness: {correct}/{total} verdicts correct ({acc:.1} %), wrongly kept: {wrong_keeps}, sessions median {} max {}", sessions[10], sessions[19]);
    assert!(acc >= 95.0, "accuracy {acc:.1} %");
    assert_eq!(
        wrong_keeps, 0,
        "a useless or harmful tweak must never be kept"
    );
}
