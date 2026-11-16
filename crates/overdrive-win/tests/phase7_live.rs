#![allow(clippy::permissions_set_readonly_false)] // Windows-only test: clears the read-only attribute it set
//! Phase 7 against the real machine: atomic file patches on the real disk and NVAPI DRS on
//! the real driver. The driver test only touches a made-up executable
//! (`overdrive-selftest.exe`) and removes its profile again; it is skipped without an
//! NVIDIA driver, and reports (doesn't fail) when DRS writes need administrator rights.

use overdrive_core::config::{ConfigEdit, ConfigFormat};
use overdrive_core::engine::journal::{Journal, TxKind};
use overdrive_core::engine::tx::{Engine, PlannedAction};
use overdrive_core::engine::{Action, DriverSetting, DriverTarget, SysError, SystemOps};
use overdrive_win::nvapi;
use overdrive_win::system_ops::WinSystem;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("overdrive-p7-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn real_file_patch_is_atomic_and_reverts_byte_identical() {
    let d = temp_dir("file");
    let path = d.join("Engine.ini");
    let p = path.to_str().unwrap().to_string();
    let mut original = vec![0xEF, 0xBB, 0xBF];
    original.extend_from_slice(b"[SystemSettings]\r\nr.MotionBlurQuality=4\r\n\r\n[Core.Log]\r\nLogX=Off");
    std::fs::write(&path, &original).unwrap();

    let sys = WinSystem;
    let j = Journal::open_in_memory("live").unwrap();
    let plan = vec![PlannedAction::new(
        "unreal_engine_ini",
        Action::FilePatch {
            path: p.clone(),
            format: ConfigFormat::Ini,
            edits: vec![ConfigEdit {
                section: Some("SystemSettings".into()),
                key: "r.MotionBlurQuality".into(),
                value: "0".into(),
            }],
        },
    )];
    let tx = Engine::new(&j, &sys).apply(TxKind::Session, None, "live", &plan).unwrap();
    let patched = std::fs::read(&path).unwrap();
    assert_eq!(&patched[..3], &[0xEF, 0xBB, 0xBF], "BOM kept");
    assert!(String::from_utf8_lossy(&patched).contains("r.MotionBlurQuality=0\r\n"));
    assert!(!d.join("Engine.ini.overdrive.tmp").exists(), "no temp file left");

    assert!(Engine::new(&j, &sys).revert(tx).unwrap().ok());
    assert_eq!(std::fs::read(&path).unwrap(), original, "byte-identical on the real disk");

    // Read-only is respected: the engine refuses, the file and its flag stay as they are.
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&path, perm.clone()).unwrap();
    assert!(matches!(sys.file_write_atomic(&p, b"x"), Err(SysError::AccessDenied(_))));
    assert!(Engine::new(&j, &sys).apply(TxKind::Session, None, "live", &plan).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
    perm.set_readonly(false);
    std::fs::set_permissions(&path, perm).unwrap();

    // Writing many times never leaves a partial file.
    for i in 0..50 {
        let body = format!("k={i}\n").repeat(1000);
        sys.file_write_atomic(&p, body.as_bytes()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body.as_bytes());
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn real_nvidia_driver_profile_round_trip() {
    if !nvapi::available() {
        println!("SKIP: no NVIDIA driver with DRS on this machine");
        return;
    }
    const EXE: &str = "overdrive-selftest.exe";
    let sys = WinSystem;
    // Leftover from an interrupted earlier run: clean it first.
    let _ = sys.driver_clear(EXE, DriverSetting::MaxFrameRate);
    let _ = sys.driver_clear(EXE, DriverSetting::PowerMode);
    assert_eq!(nvapi::profile_name(EXE).unwrap(), None, "the test exe has no profile");
    assert_eq!(sys.driver_get(EXE, DriverSetting::MaxFrameRate).unwrap(), None);

    // Reading a real game's profile never fails (javaw.exe is in NVIDIA's own profiles).
    println!("javaw.exe profile: {:?}", nvapi::profile_name("javaw.exe"));
    println!("javaw.exe power mode user value: {:?}", sys.driver_get("javaw.exe", DriverSetting::PowerMode));

    let j = Journal::open_in_memory("live").unwrap();
    let plan = vec![
        PlannedAction::new(
            "frame_cap",
            Action::DriverSetting { exe: EXE.into(), setting: DriverSetting::MaxFrameRate, target: DriverTarget::Set(117) },
        ),
        PlannedAction::new(
            "driver_power",
            Action::DriverSetting { exe: EXE.into(), setting: DriverSetting::PowerMode, target: DriverTarget::Set(1) },
        ),
    ];
    let tx = match Engine::new(&j, &sys).apply(TxKind::Session, None, "live", &plan) {
        Ok(tx) => tx,
        Err(e) if e.to_string().contains("administrator") => {
            println!("SKIP (writes): NVAPI DRS needs administrator here — the service runs as LocalSystem: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    assert_eq!(sys.driver_get(EXE, DriverSetting::MaxFrameRate).unwrap(), Some(117));
    assert_eq!(sys.driver_get(EXE, DriverSetting::PowerMode).unwrap(), Some(1));
    assert_eq!(nvapi::profile_name(EXE).unwrap().as_deref(), Some("Overdrive - overdrive-selftest.exe"));

    let r = Engine::new(&j, &sys).revert(tx).unwrap();
    assert!(r.ok(), "{r:?}");
    assert_eq!(sys.driver_get(EXE, DriverSetting::MaxFrameRate).unwrap(), None);
    assert_eq!(sys.driver_get(EXE, DriverSetting::PowerMode).unwrap(), None);
    assert_eq!(nvapi::profile_name(EXE).unwrap(), None, "our empty profile was removed: no trace left");
}

#[test]
fn real_minecraft_plan_is_previewed_without_touching_anything() {
    use overdrive_core::catalog::{preview, Build, Catalog, GameTarget, PlanContext};
    use overdrive_core::engine::RegRoot;
    use overdrive_core::games::UserDirs;
    use overdrive_core::safety::{AntiCheat, SafetyPolicy};

    let Ok(appdata) = std::env::var("APPDATA") else { return };
    let options = format!(r"{appdata}\.minecraft\options.txt");
    let Ok(before) = std::fs::read(&options) else {
        println!("SKIP: Minecraft Java not installed");
        return;
    };
    let catalog = Catalog::builtin();
    let dirs = UserDirs { appdata: Some(appdata), local_appdata: std::env::var("LOCALAPPDATA").ok(), documents: None };
    let hw = overdrive_core::build_profile(&overdrive_core::testing::FakeSource::desktop_entry());
    let policy = SafetyPolicy::default();
    let game = GameTarget {
        id: "minecraft:java".into(),
        exe_path: String::new(),
        exe_name: "javaw.exe".into(),
        anticheat: AntiCheat::None,
        install_dir: String::new(),
    };
    for expert in [false, true] {
        let ctx = PlanContext {
            game: &game, hw: &hw, user_root: RegRoot::CurrentUser, procs: None, policy: &policy,
            capabilities: &[], now_hour: 20, games: &catalog.games, dirs: &dirs, baseline: None, expert,
        };
        match catalog.build("game_config", &ctx, &WinSystem).unwrap() {
            Build::Actions(a) => {
                for l in preview(&a, &WinSystem) {
                    println!("{} {}: {:?} -> {}", if expert { "expert" } else { "easy  " }, l.key, l.from, l.to);
                }
            }
            other => println!("{}: {other:?}", if expert { "expert" } else { "easy" }),
        }
    }
    assert_eq!(std::fs::read(&options).unwrap(), before, "planning never writes");
}
