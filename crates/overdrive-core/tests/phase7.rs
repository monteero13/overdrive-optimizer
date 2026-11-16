//! Phase 7 — game config patcher and driver profiles, against the in-memory FakeSystem.
//! DoD: original files restored byte-identical on revert; protected titles never touched.

use overdrive_core::build_profile;
use overdrive_core::catalog::{preview, Build, Catalog, GameTarget, PlanContext, UNREAL_EDITS};
use overdrive_core::config::{ConfigEdit, ConfigFormat};
use overdrive_core::engine::fake::{FakeState, FakeSystem};
use overdrive_core::engine::journal::{Journal, StepState, TxKind};
use overdrive_core::engine::tx::{Engine, PlannedAction, TxError};
use overdrive_core::engine::{Action, DriverSetting, DriverTarget, RegRoot};
use overdrive_core::games::{GameAntiCheat, GameDb, UserDirs};
use overdrive_core::safety::{AcKind, AntiCheat, SafetyPolicy};
use overdrive_core::session::{Exclusion, Orchestrator};
use overdrive_core::telemetry::metrics::SessionMetrics;
use overdrive_core::testing::FakeSource;

const APPDATA: &str = r"C:\Users\a\AppData\Roaming";
const LOCAL: &str = r"C:\Users\a\AppData\Local";
const MC_OPTIONS: &str = r"C:\Users\a\AppData\Roaming\.minecraft\options.txt";
const JAVAW: &str = r"C:\Users\a\AppData\Local\Packages\Microsoft.4297127D64EC6_8wekyb3d8bbwe\LocalCache\Local\runtime\java-runtime-gamma\windows-x64\java-runtime-gamma\bin\javaw.exe";

fn dirs() -> UserDirs {
    UserDirs {
        appdata: Some(APPDATA.into()),
        local_appdata: Some(LOCAL.into()),
        documents: None,
    }
}

/// A real-looking options.txt (CRLF, like the one on the dev machine) with the heavy
/// chunk builder ("Nearby") and default biome blend.
fn mc_options() -> Vec<u8> {
    b"version:3824\r\nbiomeBlendRadius:2\r\nenableVsync:false\r\nentityShadows:true\r\nprioritizeChunkUpdates:2\r\nmaxFps:260\r\nrenderDistance:12\r\n".to_vec()
}

fn minecraft() -> GameTarget {
    GameTarget {
        id: "minecraft:java".into(),
        exe_path: JAVAW.into(),
        exe_name: "javaw.exe".into(),
        anticheat: AntiCheat::None,
        install_dir: String::new(),
    }
}

fn state_with(files: &[(&str, Vec<u8>, bool)]) -> FakeState {
    let mut s = FakeState::default();
    for (p, b, ro) in files {
        s.files.insert(p.to_ascii_lowercase(), (b.clone(), *ro));
    }
    s
}

fn file(sys: &FakeSystem, path: &str) -> Option<Vec<u8>> {
    sys.snapshot().files.get(&path.to_ascii_lowercase()).map(|(b, _)| b.clone())
}

struct Ctx {
    catalog: Catalog,
    dirs: UserDirs,
    policy: SafetyPolicy,
    hw: overdrive_core::HardwareProfile,
}

impl Ctx {
    fn new() -> Self {
        Self {
            catalog: Catalog::builtin(),
            dirs: dirs(),
            policy: SafetyPolicy::default(),
            hw: build_profile(&FakeSource::desktop_entry()),
        }
    }
    fn plan<'a>(&'a self, game: &'a GameTarget, expert: bool, baseline: Option<overdrive_core::catalog::Baseline>) -> PlanContext<'a> {
        PlanContext {
            game,
            hw: &self.hw,
            user_root: RegRoot::CurrentUser,
            procs: None,
            policy: &self.policy,
            capabilities: &[],
            now_hour: 20,
            games: &self.catalog.games,
            dirs: &self.dirs,
            baseline,
            expert,
        }
    }
}

fn actions(b: Build) -> Vec<Action> {
    match b {
        Build::Actions(a) => a,
        other => panic!("expected actions, got {other:?}"),
    }
}

fn apply(sys: &FakeSystem, j: &Journal, id: &str, a: Vec<Action>) -> Result<i64, TxError> {
    let plan: Vec<PlannedAction> = a.into_iter().map(|a| PlannedAction::new(id, a)).collect();
    Engine::new(j, sys).apply(TxKind::Session, Some("g"), "test", &plan)
}

// ------------------------------------------------------------------ game config (Minecraft)

#[test]
fn minecraft_easy_mode_patches_only_proven_low_impact_settings() {
    let c = Ctx::new();
    let g = minecraft();
    let sys = FakeSystem::new(state_with(&[(MC_OPTIONS, mc_options(), false)]));

    let a = actions(c.catalog.build("game_config", &c.plan(&g, false, None), &sys).unwrap());
    let Action::FilePatch { path, format, edits } = &a[0] else { panic!("{a:?}") };
    assert_eq!(path, MC_OPTIONS);
    assert_eq!(*format, ConfigFormat::Colon);
    let keys: Vec<(&str, &str)> = edits.iter().map(|e| (e.key.as_str(), e.value.as_str())).collect();
    assert_eq!(keys, vec![("prioritizeChunkUpdates", "0"), ("biomeBlendRadius", "1")]);

    // Expert also tests the disputed one.
    let a = actions(c.catalog.build("game_config", &c.plan(&g, true, None), &sys).unwrap());
    let Action::FilePatch { edits, .. } = &a[0] else { panic!() };
    assert!(edits.iter().any(|e| e.key == "entityShadows" && e.value == "false"));

    // Apply → exact bytes; revert → the original, byte for byte.
    let j = Journal::open_in_memory("t").unwrap();
    let tx = apply(&sys, &j, "game_config", a).unwrap();
    assert_eq!(
        file(&sys, MC_OPTIONS).unwrap(),
        b"version:3824\r\nbiomeBlendRadius:1\r\nenableVsync:false\r\nentityShadows:false\r\nprioritizeChunkUpdates:0\r\nmaxFps:260\r\nrenderDistance:12\r\n"
    );
    assert!(Engine::new(&j, &sys).revert(tx).unwrap().ok());
    assert_eq!(file(&sys, MC_OPTIONS).unwrap(), mc_options(), "byte-identical");
}

#[test]
fn a_player_who_already_chose_lighter_values_is_left_alone() {
    let c = Ctx::new();
    let g = minecraft();
    let light = b"biomeBlendRadius:0\nprioritizeChunkUpdates:0\nentityShadows:false\n".to_vec();
    let sys = FakeSystem::new(state_with(&[(MC_OPTIONS, light, false)]));
    // Never raise biome blend 0 → 1.
    assert_eq!(c.catalog.build("game_config", &c.plan(&g, true, None), &sys).unwrap(), Build::AlreadyOptimal);
}

#[test]
fn the_game_rewriting_the_file_mid_session_keeps_the_players_changes() {
    let c = Ctx::new();
    let g = minecraft();
    let sys = FakeSystem::new(state_with(&[(MC_OPTIONS, mc_options(), false)]));
    let a = actions(c.catalog.build("game_config", &c.plan(&g, false, None), &sys).unwrap());
    let j = Journal::open_in_memory("t").unwrap();
    let tx = apply(&sys, &j, "game_config", a).unwrap();

    // In-game the player raises render distance; Minecraft rewrites options.txt (LF now),
    // keeping our two values.
    let rewritten = b"version:3824\nbiomeBlendRadius:1\nenableVsync:false\nentityShadows:true\nprioritizeChunkUpdates:0\nmaxFps:260\nrenderDistance:20\n".to_vec();
    sys.state.borrow_mut().files.insert(MC_OPTIONS.to_ascii_lowercase(), (rewritten, false));

    let r = Engine::new(&j, &sys).revert(tx).unwrap();
    assert!(r.ok(), "{r:?}");
    // Our keys are back to the originals; the player's render distance survives.
    assert_eq!(
        file(&sys, MC_OPTIONS).unwrap(),
        b"version:3824\nbiomeBlendRadius:2\nenableVsync:false\nentityShadows:true\nprioritizeChunkUpdates:2\nmaxFps:260\nrenderDistance:20\n"
    );
}

#[test]
fn values_the_player_changed_after_us_are_theirs() {
    let c = Ctx::new();
    let g = minecraft();
    let sys = FakeSystem::new(state_with(&[(MC_OPTIONS, mc_options(), false)]));
    let a = actions(c.catalog.build("game_config", &c.plan(&g, false, None), &sys).unwrap());
    let j = Journal::open_in_memory("t").unwrap();
    let tx = apply(&sys, &j, "game_config", a).unwrap();

    // The player moved both settings themselves → superseded, file left exactly as they set it.
    let theirs = b"biomeBlendRadius:5\nprioritizeChunkUpdates:1\n".to_vec();
    sys.state.borrow_mut().files.insert(MC_OPTIONS.to_ascii_lowercase(), (theirs.clone(), false));
    let r = Engine::new(&j, &sys).revert(tx).unwrap();
    assert_eq!(r.superseded, vec!["game_config".to_string()]);
    assert_eq!(file(&sys, MC_OPTIONS).unwrap(), theirs);

    // Mixed: one of ours still holds, the other the player changed → only ours is undone.
    let sys = FakeSystem::new(state_with(&[(MC_OPTIONS, mc_options(), false)]));
    let a = actions(c.catalog.build("game_config", &c.plan(&g, false, None), &sys).unwrap());
    let j = Journal::open_in_memory("t").unwrap();
    let tx = apply(&sys, &j, "game_config", a).unwrap();
    let mixed = b"biomeBlendRadius:1\nprioritizeChunkUpdates:1\n".to_vec();
    sys.state.borrow_mut().files.insert(MC_OPTIONS.to_ascii_lowercase(), (mixed, false));
    assert!(Engine::new(&j, &sys).revert(tx).unwrap().ok());
    assert_eq!(file(&sys, MC_OPTIONS).unwrap(), b"biomeBlendRadius:2\nprioritizeChunkUpdates:1\n");
}

#[test]
fn read_only_files_are_respected() {
    let c = Ctx::new();
    let g = minecraft();
    let sys = FakeSystem::new(state_with(&[(MC_OPTIONS, mc_options(), true)]));
    assert_eq!(
        c.catalog.build("game_config", &c.plan(&g, false, None), &sys).unwrap(),
        Build::NotApplicable("config_read_only")
    );
    // Even a hand-made plan cannot write it: the step fails, nothing changes.
    let edit = ConfigEdit { section: None, key: "maxFps".into(), value: "120".into() };
    let a = vec![Action::FilePatch { path: MC_OPTIONS.into(), format: ConfigFormat::Colon, edits: vec![edit] }];
    let j = Journal::open_in_memory("t").unwrap();
    assert!(matches!(apply(&sys, &j, "x", a), Err(TxError::StepFailed { rolled_back: true, .. })));
    assert_eq!(file(&sys, MC_OPTIONS).unwrap(), mc_options());
    assert!(sys.snapshot().files[&MC_OPTIONS.to_ascii_lowercase()].1, "still read-only");
}

#[test]
fn missing_or_unreadable_files_are_never_created_or_mangled() {
    let c = Ctx::new();
    let g = minecraft();
    let sys = FakeSystem::new(FakeState::default());
    assert_eq!(
        c.catalog.build("game_config", &c.plan(&g, false, None), &sys).unwrap(),
        Build::NotApplicable("config_not_found")
    );
    // Not valid UTF-8 (e.g. a Latin-1 file): the builder skips it rather than guessing.
    let sys = FakeSystem::new(state_with(&[(MC_OPTIONS, b"lang:espa\xF1ol\nprioritizeChunkUpdates:2\n".to_vec(), false)]));
    assert_eq!(c.catalog.build("game_config", &c.plan(&g, false, None), &sys).unwrap(), Build::AlreadyOptimal);
}

// ------------------------------------------------------------------ protected titles

#[test]
fn protected_titles_are_never_touched() {
    let c = Ctx::new();
    let db = GameDb::builtin();
    // Every config file a builder could look for exists for every protected title.
    let mut st = state_with(&[(MC_OPTIONS, mc_options(), false)]);
    let unreal = format!(r"{LOCAL}\FortniteGame\Saved\Config\Windows");
    for f in ["GameUserSettings.ini", "Engine.ini"] {
        st.files.insert(format!(r"{unreal}\{f}").to_ascii_lowercase(), (b"[SystemSettings]\r\n".to_vec(), false));
    }
    st.driver_profiles = true;
    let sys = FakeSystem::new(st);
    let baseline = overdrive_core::catalog::Baseline { avg_fps: 160.0, low_1: 100.0, cpu_bound: true, runs: 3 };

    let protected: Vec<&str> = db
        .games
        .iter()
        .filter(|g| g.anticheat == GameAntiCheat::Protected)
        .flat_map(|g| g.library_ids.iter().map(String::as_str))
        .collect();
    assert!(protected.contains(&"steam:730") && protected.contains(&"epic:fortnite"));

    let mut targets: Vec<GameTarget> = protected
        .iter()
        .map(|id| GameTarget {
            id: (*id).into(),
            exe_path: r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe".into(),
            exe_name: "FortniteClient-Win64-Shipping.exe".into(),
            // Detection failed on this machine: the database alone must still block it.
            anticheat: AntiCheat::None,
            install_dir: String::new(),
        })
        .collect();
    // And a game the database doesn't know, but whose anti-cheat was detected.
    targets.push(GameTarget { id: "minecraft:java".into(), anticheat: AntiCheat::UserMode(AcKind::Vac), ..minecraft() });
    targets.push(GameTarget { id: "steam:999".into(), anticheat: AntiCheat::Unknown, ..targets[0].clone() });

    for g in &targets {
        for expert in [false, true] {
            for t in &c.catalog.tweaks {
                let b = c.catalog.build(&t.id, &c.plan(g, expert, Some(baseline)), &sys).unwrap();
                if let Build::Actions(a) = b {
                    assert!(
                        !a.iter().any(|a| matches!(a, Action::FilePatch { .. })),
                        "{}: {} would patch a file of a protected title: {a:?}",
                        g.id,
                        t.id
                    );
                }
            }
        }
    }
}

#[test]
fn the_orchestrator_never_patches_a_protected_game_end_to_end() {
    let catalog = Catalog::builtin();
    let mut st = FakeState::default();
    let unreal = format!(r"{LOCAL}\FortniteGame\Saved\Config\Windows");
    for f in ["GameUserSettings.ini", "Engine.ini"] {
        st.files.insert(format!(r"{unreal}\{f}").to_ascii_lowercase(), (b"[SystemSettings]\r\nr.MotionBlurQuality=4\r\n".to_vec(), false));
    }
    let before = st.files.clone();
    let sys = FakeSystem::new(st);
    let j = Journal::open_in_memory("t").unwrap();
    let hw = build_profile(&FakeSource::desktop_entry());
    let mut o = Orchestrator::new(&j, &sys, &catalog);
    o.expert = true;
    o.dirs = dirs();
    let g = GameTarget {
        id: "epic:fortnite".into(),
        exe_path: r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe".into(),
        exe_name: "FortniteClient-Win64-Shipping.exe".into(),
        anticheat: AntiCheat::Kernel(AcKind::EasyAntiCheat),
        install_dir: String::new(),
    };
    let (cands, excl) = o.candidates(&g, &hw).unwrap();
    for id in ["game_config", "unreal_engine_ini"] {
        assert!(!cands.contains(&id.to_string()), "{id} must not be a candidate");
        assert!(excl.iter().any(|(t, e)| t == id && matches!(e, Exclusion::AntiCheat | Exclusion::NotApplicable(_))));
    }
    let armed = o.arm(&g, &hw).unwrap();
    assert!(armed.plan.testing.as_deref() != Some("game_config"));
    assert_eq!(sys.snapshot().files, before, "no file of a protected title changed");
}

// ------------------------------------------------------------------ driver profiles + frame cap

fn record(j: &Journal, game: &str, avg: f64, low1: f64, bottleneck: &str) {
    let m = SessionMetrics {
        frames: 10_000,
        gameplay_secs: 120.0,
        avg_fps: avg,
        low_1: low1,
        low_01: low1 * 0.8,
        p50_ms: 1000.0 / avg,
        p95_ms: 1000.0 / (low1 * 1.2),
        p99_ms: 1000.0 / low1,
        stdev_ms: 2.5,
        stutters: 3,
    };
    j.record_session(None, game, &[], &serde_json::json!({}), &m, Some(bottleneck), true).unwrap();
}

#[test]
fn frame_cap_uses_the_driver_when_it_can_and_the_game_file_when_it_cannot() {
    let catalog = Catalog::builtin();
    let hw = build_profile(&FakeSource::desktop_entry());
    let mut st = state_with(&[(MC_OPTIONS, mc_options(), false)]);
    st.driver_profiles = true;
    let sys = FakeSystem::new(st);
    let j = Journal::open_in_memory("t").unwrap();
    let mut o = Orchestrator::new(&j, &sys, &catalog);
    o.dirs = dirs();

    let game = GameTarget {
        id: "steam:1".into(),
        exe_path: r"C:\Games\G\g.exe".into(),
        exe_name: "g.exe".into(),
        anticheat: AntiCheat::Kernel(AcKind::BattlEye),
        install_dir: r"C:\Games\G".into(),
    };
    let policy = SafetyPolicy::default();
    let build = |g: &GameTarget| catalog.build("frame_cap", &o.ctx(g, &hw, None, &policy), &sys).unwrap();

    assert_eq!(build(&game), Build::NotApplicable("needs_baseline"));
    record(&j, "steam:1", 150.0, 93.0, "gpu_bound");
    record(&j, "steam:1", 152.0, 95.0, "gpu_bound");
    assert_eq!(build(&game), Build::NotApplicable("not_cpu_bound"));
    for _ in 0..3 {
        record(&j, "steam:1", 150.0, 94.0, "cpu_bound");
    }
    // median low_1 = 94 → 94 × 0.97 = 91.2 → 90 (steps of 5).
    // Driver limiter: safe even with a kernel anti-cheat (nothing touches the game).
    assert_eq!(
        build(&game),
        Build::Actions(vec![Action::DriverSetting {
            exe: "g.exe".into(),
            setting: DriverSetting::MaxFrameRate,
            target: DriverTarget::Set(90)
        }])
    );
    // The player already caps lower in the driver: leave it.
    sys.state.borrow_mut().driver.insert(("g.exe".into(), DriverSetting::MaxFrameRate), 60);
    assert_eq!(build(&game), Build::AlreadyOptimal);

    // Minecraft (javaw.exe): a driver cap would hit every Java program → the game's own limiter.
    for _ in 0..3 {
        record(&j, "minecraft:java", 200.0, 110.0, "cpu_bound");
    }
    let a = actions(build(&minecraft()));
    assert_eq!(
        a,
        vec![Action::FilePatch {
            path: MC_OPTIONS.into(),
            format: ConfigFormat::Colon,
            edits: vec![ConfigEdit { section: None, key: "maxFps".into(), value: "105".into() }],
        }]
    );
    // driver_power and IFEO priority also refuse the shared executable.
    for id in ["driver_power", "game_priority"] {
        assert_eq!(
            catalog.build(id, &o.ctx(&minecraft(), &hw, None, &policy), &sys).unwrap(),
            Build::NotApplicable("shared_executable"),
            "{id}"
        );
    }

    // A cap below 60 or without headroom is never proposed.
    let low = overdrive_core::catalog::Baseline { avg_fps: 70.0, low_1: 50.0, cpu_bound: true, runs: 3 };
    assert_eq!(low.frame_cap(), None);
    let flat = overdrive_core::catalog::Baseline { avg_fps: 100.0, low_1: 98.0, cpu_bound: true, runs: 3 };
    assert_eq!(flat.frame_cap(), None);
}

#[test]
fn driver_settings_revert_to_exactly_what_was_there() {
    let mut st = FakeState { driver_profiles: true, ..Default::default() };
    st.driver.insert(("a.exe".into(), DriverSetting::PowerMode), 0);
    let original = st.clone();
    let sys = FakeSystem::new(st);
    let j = Journal::open_in_memory("t").unwrap();
    let a = vec![
        Action::DriverSetting { exe: "a.exe".into(), setting: DriverSetting::PowerMode, target: DriverTarget::Set(1) },
        Action::DriverSetting { exe: "b.exe".into(), setting: DriverSetting::PowerMode, target: DriverTarget::Set(1) },
    ];
    let tx = apply(&sys, &j, "driver_power", a).unwrap();
    assert_eq!(sys.snapshot().driver.len(), 2);
    assert!(Engine::new(&j, &sys).revert(tx).unwrap().ok());
    assert_eq!(sys.snapshot(), original, "a.exe back to 0, b.exe back to no user value");

    // Someone changes the value after us (NVIDIA app, the user): never overwritten.
    let tx = apply(&sys, &j, "driver_power", vec![Action::DriverSetting {
        exe: "b.exe".into(),
        setting: DriverSetting::MaxFrameRate,
        target: DriverTarget::Set(90),
    }])
    .unwrap();
    sys.state.borrow_mut().driver.insert(("b.exe".into(), DriverSetting::MaxFrameRate), 141);
    let r = Engine::new(&j, &sys).revert(tx).unwrap();
    assert_eq!(r.superseded.len(), 1);
    assert_eq!(sys.snapshot().driver[&("b.exe".into(), DriverSetting::MaxFrameRate)], 141);
    assert!(j.steps(tx).unwrap().iter().all(|s| s.state == StepState::Superseded));
}

#[test]
fn without_an_nvidia_driver_the_driver_tweaks_do_not_apply() {
    let c = Ctx::new();
    let sys = FakeSystem::new(FakeState::default());
    let g = GameTarget { id: "steam:1".into(), exe_path: r"C:\G\g.exe".into(), exe_name: "g.exe".into(), anticheat: AntiCheat::None, install_dir: String::new() };
    assert_eq!(c.catalog.build("driver_power", &c.plan(&g, true, None), &sys).unwrap(), Build::NotApplicable("no_driver_profiles"));
    let b = overdrive_core::catalog::Baseline { avg_fps: 160.0, low_1: 100.0, cpu_bound: true, runs: 3 };
    assert_eq!(c.catalog.build("frame_cap", &c.plan(&g, true, Some(b)), &sys).unwrap(), Build::NotApplicable("no_frame_limiter"));
}

// ------------------------------------------------------------------ generic Unreal rule

#[test]
fn unreal_rule_previews_exactly_what_it_changes_and_never_creates_files() {
    let c = Ctx::new();
    let dir = format!(r"{LOCAL}\Phoenix\Saved\Config\WindowsNoEditor");
    let g = GameTarget {
        id: "steam:990080".into(),
        exe_path: r"D:\Steam\steamapps\common\Hogwarts Legacy\Phoenix\Binaries\Win64\HogwartsLegacy.exe".into(),
        exe_name: "HogwartsLegacy.exe".into(),
        anticheat: AntiCheat::None,
        install_dir: String::new(),
    };
    let engine = format!(r"{dir}\Engine.ini");
    let original = b"[Core.System]\r\nPaths=../../Content\r\n\r\n[SystemSettings]\r\nr.MotionBlurQuality=0\r\n".to_vec();
    let sys = FakeSystem::new(state_with(&[
        (&format!(r"{dir}\GameUserSettings.ini"), b"[/Script/Engine.GameUserSettings]\r\n".to_vec(), false),
        (&engine, original.clone(), false),
    ]));
    let a = actions(c.catalog.build("unreal_engine_ini", &c.plan(&g, true, None), &sys).unwrap());
    let p = preview(&a, &sys);
    // Motion blur was already off: only the other two appear, with their current value.
    assert_eq!(p.len(), UNREAL_EDITS.len() - 1);
    assert!(p.iter().all(|l| l.from.is_none() && l.to == "0" && l.file == engine));
    assert_eq!(p[0].key, "r.SceneColorFringeQuality");

    let j = Journal::open_in_memory("t").unwrap();
    let tx = apply(&sys, &j, "unreal_engine_ini", a).unwrap();
    assert_eq!(
        String::from_utf8(file(&sys, &engine).unwrap()).unwrap(),
        "[Core.System]\r\nPaths=../../Content\r\n\r\n[SystemSettings]\r\nr.MotionBlurQuality=0\r\nr.SceneColorFringeQuality=0\r\nr.VolumetricFog=0\r\n"
    );
    assert!(Engine::new(&j, &sys).revert(tx).unwrap().ok());
    assert_eq!(file(&sys, &engine).unwrap(), original, "byte-identical");

    // No Engine.ini → nothing (we never create config files).
    let sys = FakeSystem::new(state_with(&[(&format!(r"{dir}\GameUserSettings.ini"), vec![], false)]));
    assert_eq!(c.catalog.build("unreal_engine_ini", &c.plan(&g, true, None), &sys).unwrap(), Build::NotApplicable("config_not_found"));
    // Easy mode never sees it (medium risk, disputed).
    assert!(!c.catalog.get("unreal_engine_ini").unwrap().easy_mode_ok());
}
