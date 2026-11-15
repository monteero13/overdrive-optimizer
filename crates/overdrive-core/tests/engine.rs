//! Phase 5 — transaction engine + safety policy, against the in-memory FakeSystem.
//! (The real-process kill test against the real registry lives in overdrive-win.)

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use overdrive_core::engine::fake::{FakeProc, FakeState, FakeSystem, Fault};
use overdrive_core::engine::journal::{Journal, StepState, TxKind, TxState};
use overdrive_core::engine::tx::{CrashPoint, Engine, PlannedAction, TxError};
use overdrive_core::engine::{
    Action, PriorityClass, ProcRef, RegData, RegRoot, RegTarget, StartType, SystemOps,
};
use overdrive_core::safety::{
    anticheat_status, AcKind, AntiCheat, BackgroundDecision, GameInfo, NeverTouch, ProcInfo,
    SafetyPolicy,
};

use overdrive_core::safety::NeverTouch as NT;

const HKCU: RegRoot = RegRoot::CurrentUser;
const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const HIGH: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";

fn p(pid: u32, image: &str) -> ProcRef {
    ProcRef {
        pid,
        start_time: 1000 + pid as u64,
        image: image.into(),
    }
}

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

fn original() -> FakeState {
    let mut s = FakeState {
        power: BALANCED.into(),
        ..Default::default()
    };
    s.add_reg(
        &HKCU,
        "System\\GameConfigStore",
        "GameDVR_Enabled",
        RegData::Dword(1),
    );
    s.add_reg(
        &HKCU,
        "Software\\Test",
        "keepme",
        RegData::String("hello".into()),
    );
    // Software\Microsoft exists; Software\Microsoft\DirectX\UserGpuPreferences does not:
    // applying gpu_pref creates two keys that revert must remove again.
    s.add_reg(
        &HKCU,
        "Software\\Microsoft\\Office",
        "other",
        RegData::Dword(7),
    );
    s.services.insert("DiagTrack".into(), StartType::Automatic);
    s.procs.extend([
        proc_state(100, "chrome.exe"),
        proc_state(200, "GoogleUpdate.exe"),
        proc_state(300, "game.exe"),
    ]);
    s
}

/// One action of every kind, including "create a value that didn't exist" and "delete one that did".
fn full_plan() -> Vec<PlannedAction> {
    vec![
        PlannedAction::new(
            "gamedvr_off",
            Action::RegistryValue {
                root: HKCU,
                key: "System\\GameConfigStore".into(),
                name: "GameDVR_Enabled".into(),
                target: RegTarget::Set(RegData::Dword(0)),
            },
        ),
        PlannedAction::new(
            "gpu_pref",
            Action::RegistryValue {
                root: HKCU,
                key: "Software\\Microsoft\\DirectX\\UserGpuPreferences".into(),
                name: "C:\\Games\\game.exe".into(),
                target: RegTarget::Set(RegData::String("GpuPreference=2;".into())),
            },
        ),
        PlannedAction::new(
            "delete_test",
            Action::RegistryValue {
                root: HKCU,
                key: "Software\\Test".into(),
                name: "keepme".into(),
                target: RegTarget::Delete,
            },
        ),
        PlannedAction::new(
            "telemetry",
            Action::ServiceStart {
                name: "DiagTrack".into(),
                start: StartType::Manual,
            },
        ),
        PlannedAction::new("power", Action::PowerScheme { guid: HIGH.into() }),
        PlannedAction::new(
            "bg_eco",
            Action::ProcessEcoQos {
                process: p(100, "chrome.exe"),
                on: true,
            },
        ),
        PlannedAction::new(
            "bg_prio",
            Action::ProcessPriority {
                process: p(100, "chrome.exe"),
                class: PriorityClass::BelowNormal,
            },
        ),
        PlannedAction::new(
            "bg_suspend",
            Action::ProcessSuspend {
                process: p(200, "GoogleUpdate.exe"),
            },
        ),
    ]
}

fn temp_db(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("overdrive-test-{}-{}", std::process::id(), name));
    let _ = std::fs::create_dir_all(&d);
    let f = d.join("journal.db");
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", f.display(), ext));
    }
    f
}

#[test]
fn apply_then_revert_restores_everything_exactly() {
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("test").unwrap();
    let e = Engine::new(&j, &sys);
    let tx = e
        .apply(TxKind::Session, Some("steam:1"), "game_start", &full_plan())
        .unwrap();
    let applied = sys.snapshot();
    assert_ne!(applied, original());
    assert_eq!(applied.power, HIGH);
    assert!(applied.procs[&200].suspended);
    assert!(!applied.reg.contains_key(&(
        "CurrentUser".into(),
        "software\\test".into(),
        "keepme".into()
    )));
    assert!(
        applied.reg_keys.contains(&(
            "CurrentUser".into(),
            "software\\microsoft\\directx\\usergpupreferences".into()
        )),
        "apply created the key chain"
    );
    assert_eq!(j.tx(tx).unwrap().unwrap().state, TxState::Applied);
    assert!(j
        .steps(tx)
        .unwrap()
        .iter()
        .all(|s| s.state == StepState::Verified));

    let r = e.revert(tx).unwrap();
    assert!(r.ok(), "{r:?}");
    assert_eq!(r.reverted, 8);
    assert_eq!(sys.snapshot(), original(), "byte-identical to the original");
    assert!(j
        .steps(tx)
        .unwrap()
        .iter()
        .all(|s| s.state == StepState::RevertVerified));

    // Idempotent: reverting again changes nothing and doesn't fail.
    let again = e.revert(tx).unwrap();
    assert!(again.ok());
    assert_eq!(sys.snapshot(), original());
}

#[test]
fn a_failing_step_rolls_back_the_previous_ones() {
    for (k, op) in [
        (0usize, "reg_write"),
        (0, "service_write"),
        (0, "power_write"),
        (0, "proc_suspend"),
    ] {
        let sys = FakeSystem::new(original());
        sys.fault(Fault::FailNth(op, k));
        let j = Journal::open_in_memory("test").unwrap();
        let err = Engine::new(&j, &sys)
            .apply(TxKind::Session, None, "t", &full_plan())
            .unwrap_err();
        assert!(
            matches!(
                err,
                TxError::StepFailed {
                    rolled_back: true,
                    ..
                }
            ),
            "{op}: {err}"
        );
        assert_eq!(sys.snapshot(), original(), "{op}: nothing left applied");
        let tx = j.txs_in(&[TxState::Reverted]).unwrap();
        assert_eq!(tx.len(), 1, "{op}: transaction closed as reverted");
    }
}

#[test]
fn a_change_that_silently_does_nothing_is_caught_by_verification() {
    let sys = FakeSystem::new(original());
    sys.fault(Fault::Silent("power_write"));
    let j = Journal::open_in_memory("test").unwrap();
    let err = Engine::new(&j, &sys)
        .apply(TxKind::Session, None, "t", &full_plan())
        .unwrap_err();
    assert!(
        matches!(err, TxError::VerifyFailed { ref tweak_id, rolled_back: true } if tweak_id == "power"),
        "{err}"
    );
    assert_eq!(sys.snapshot(), original());
}

#[test]
fn a_value_changed_by_someone_else_is_not_overwritten() {
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("test").unwrap();
    let e = Engine::new(&j, &sys);
    let tx = e.apply(TxKind::Session, None, "t", &full_plan()).unwrap();
    // The user switches the power plan themselves during the game.
    sys.set_power_scheme("a1841308-3541-4fab-bc81-f71556f20b4a")
        .unwrap();
    let r = e.revert(tx).unwrap();
    assert_eq!(r.superseded, vec!["power".to_string()]);
    assert_eq!(
        sys.snapshot().power,
        "a1841308-3541-4fab-bc81-f71556f20b4a",
        "user's choice kept"
    );
    let mut expected = original();
    expected.power = "a1841308-3541-4fab-bc81-f71556f20b4a".into();
    assert_eq!(sys.snapshot(), expected, "everything else restored");
}

#[test]
fn revert_failures_are_reported_precisely_never_silent() {
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("test").unwrap();
    let e = Engine::new(&j, &sys);
    let tx = e.apply(TxKind::Session, None, "t", &full_plan()).unwrap();
    sys.fault(Fault::FailAlways("service_write"));
    let r = e.revert(tx).unwrap();
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].0, "telemetry");
    assert_eq!(j.tx(tx).unwrap().unwrap().state, TxState::Failed);
    // Recovery retries it later (e.g. next boot) once the cause is gone.
    sys.faults.borrow_mut().clear();
    let rec = Engine::new(&j, &sys).recover().unwrap();
    assert_eq!(rec.reverted.len(), 1);
    assert_eq!(sys.snapshot(), original());
}

/// Count every crash point a full apply + revert passes through.
fn crash_points(plan: &[PlannedAction]) -> (Vec<CrashPoint>, Vec<CrashPoint>) {
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("test").unwrap();
    let seen = std::cell::RefCell::new(Vec::new());
    let mut e = Engine::new(&j, &sys);
    e.hook = Some(Box::new(|cp| seen.borrow_mut().push(cp)));
    let tx = e.apply(TxKind::Session, None, "t", plan).unwrap();
    let apply_points = seen.borrow().clone();
    seen.borrow_mut().clear();
    e.revert(tx).unwrap();
    let revert_points = seen.borrow().clone();
    (apply_points, revert_points)
}

#[test]
fn killed_at_every_point_during_apply_recovery_restores_the_original() {
    let plan = full_plan();
    let (points, _) = crash_points(&plan);
    assert_eq!(points.len(), 2 + plan.len() * 4, "{points:?}");
    for (n, cp) in points.iter().enumerate() {
        let db = temp_db(&format!("apply-{n}"));
        let sys = FakeSystem::new(original());
        {
            let j = Journal::open(&db, "test").unwrap();
            let count = Cell::new(0usize);
            let mut e = Engine::new(&j, &sys);
            e.hook = Some(Box::new(|_| {
                if count.get() == n {
                    panic!("simulated kill at {cp:?}");
                }
                count.set(count.get() + 1);
            }));
            let r = catch_unwind(AssertUnwindSafe(|| {
                e.apply(TxKind::Session, None, "t", &plan)
            }));
            assert!(r.is_err(), "should have crashed at {cp:?}");
        } // journal connection dropped like a dead process
        let j = Journal::open(&db, "test").unwrap(); // "service restarts"
        let rep = Engine::new(&j, &sys).recover().unwrap();
        assert_eq!(
            sys.snapshot(),
            original(),
            "crash at {cp:?} (#{n}) left changes behind: {rep:?}"
        );
        assert!(rep.reverted.iter().all(|r| r.ok()), "{cp:?}: {rep:?}");
    }
}

#[test]
fn killed_at_every_point_during_revert_recovery_finishes_the_job() {
    let plan = full_plan();
    let (_, points) = crash_points(&plan);
    assert_eq!(points.len(), plan.len() * 2);
    for (n, cp) in points.iter().enumerate() {
        let db = temp_db(&format!("revert-{n}"));
        let sys = FakeSystem::new(original());
        {
            let j = Journal::open(&db, "test").unwrap();
            let tx = Engine::new(&j, &sys)
                .apply(TxKind::Session, None, "t", &plan)
                .unwrap();
            let count = Cell::new(0usize);
            let mut e = Engine::new(&j, &sys);
            e.hook = Some(Box::new(|_| {
                if count.get() == n {
                    panic!("simulated kill at {cp:?}");
                }
                count.set(count.get() + 1);
            }));
            assert!(catch_unwind(AssertUnwindSafe(|| e.revert(tx))).is_err());
        }
        let j = Journal::open(&db, "test").unwrap();
        Engine::new(&j, &sys).recover().unwrap();
        assert_eq!(sys.snapshot(), original(), "crash during revert at {cp:?}");
    }
}

#[test]
fn recovery_keeps_accepted_persistent_changes_but_undoes_half_applied_ones() {
    let db = temp_db("persistent");
    let sys = FakeSystem::new(original());
    let keep = vec![PlannedAction::new(
        "gamedvr_off",
        full_plan()[0].action.clone(),
    )];
    {
        let j = Journal::open(&db, "test").unwrap();
        Engine::new(&j, &sys)
            .apply(TxKind::Persistent, None, "user_accept", &keep)
            .unwrap();
        // A second persistent change dies half-way.
        let count = Cell::new(0usize);
        let mut e = Engine::new(&j, &sys);
        e.hook = Some(Box::new(|cp| {
            if cp == CrashPoint::AfterApply(1) {
                panic!("kill");
            }
            count.set(count.get() + 1);
        }));
        let half = vec![full_plan()[3].clone(), full_plan()[4].clone()];
        assert!(catch_unwind(AssertUnwindSafe(|| e.apply(
            TxKind::Persistent,
            None,
            "user_accept",
            &half
        )))
        .is_err());
    }
    let j = Journal::open(&db, "test").unwrap();
    Engine::new(&j, &sys).recover().unwrap();
    let s = sys.snapshot();
    assert_eq!(
        s.reg[&(
            "CurrentUser".into(),
            "system\\gameconfigstore".into(),
            "gamedvr_enabled".into()
        )],
        RegData::Dword(0),
        "accepted change kept"
    );
    assert_eq!(
        s.services["DiagTrack"],
        StartType::Automatic,
        "half-applied tx undone"
    );
    assert_eq!(s.power, BALANCED);
}

#[test]
fn global_reset_reverts_everything_and_verifies_no_diff() {
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("test").unwrap();
    let e = Engine::new(&j, &sys);
    let plan = full_plan();
    e.apply(TxKind::Persistent, None, "a", &plan[0..2]).unwrap();
    e.apply(TxKind::Persistent, None, "b", &plan[3..5]).unwrap();
    e.apply(TxKind::Session, Some("g"), "c", &plan[5..8])
        .unwrap();
    let rep = e.revert_all().unwrap();
    assert!(rep.remaining_diffs.is_empty(), "{rep:?}");
    assert!(rep.reverted.iter().all(|r| r.ok()));
    assert_eq!(sys.snapshot(), original());
    assert_eq!(
        j.txs_in(&[TxState::Applied])
            .unwrap()
            .iter()
            .filter(|t| t.kind != TxKind::GlobalReset)
            .count(),
        0
    );
}

#[test]
fn a_suspended_process_is_resumed_on_every_exit_path() {
    let suspend = vec![
        PlannedAction::new(
            "bg_suspend",
            Action::ProcessSuspend {
                process: p(200, "GoogleUpdate.exe"),
            },
        ),
        PlannedAction::new("power", Action::PowerScheme { guid: HIGH.into() }),
    ];
    let suspended = |s: &FakeSystem| s.snapshot().procs[&200].suspended;

    // 1. normal session end
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("t").unwrap();
    let e = Engine::new(&j, &sys);
    let tx = e.apply(TxKind::Session, None, "s", &suspend).unwrap();
    assert!(suspended(&sys));
    e.revert(tx).unwrap();
    assert!(!suspended(&sys), "session end");

    // 2. a later step fails → rollback
    let sys = FakeSystem::new(original());
    sys.fault(Fault::FailAlways("power_write"));
    let j = Journal::open_in_memory("t").unwrap();
    assert!(Engine::new(&j, &sys)
        .apply(TxKind::Session, None, "s", &suspend)
        .is_err());
    assert!(!suspended(&sys), "rollback");

    // 3. timeout
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("t").unwrap();
    let mut e = Engine::new(&j, &sys);
    e.deadline = Some(std::time::Duration::ZERO);
    assert!(matches!(
        e.apply(TxKind::Session, None, "s", &suspend),
        Err(TxError::TimedOut { rolled_back: true })
    ));
    assert!(!suspended(&sys), "timeout");

    // 4. crash → recover
    let db = temp_db("resume-crash");
    let sys = FakeSystem::new(original());
    {
        let j = Journal::open(&db, "t").unwrap();
        Engine::new(&j, &sys)
            .apply(TxKind::Session, None, "s", &suspend)
            .unwrap();
    }
    assert!(suspended(&sys));
    let j = Journal::open(&db, "t").unwrap();
    Engine::new(&j, &sys).recover().unwrap();
    assert!(!suspended(&sys), "recovery");

    // 5. global reset
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("t").unwrap();
    let e = Engine::new(&j, &sys);
    e.apply(TxKind::Session, None, "s", &suspend).unwrap();
    e.revert_all().unwrap();
    assert!(!suspended(&sys), "reset");

    // 6. brought to the foreground / "Resume everything" — only process actions are undone
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("t").unwrap();
    let e = Engine::new(&j, &sys);
    e.apply(TxKind::Session, None, "s", &suspend).unwrap();
    assert_eq!(
        e.release_processes(Some(&p(200, "GoogleUpdate.exe")))
            .unwrap(),
        1
    );
    assert!(!suspended(&sys), "foreground");
    assert_eq!(
        sys.snapshot().power,
        HIGH,
        "the rest of the session stays active"
    );

    // 7. the process exits on its own while suspended → revert is a no-op, not an error
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("t").unwrap();
    let e = Engine::new(&j, &sys);
    let tx = e.apply(TxKind::Session, None, "s", &suspend).unwrap();
    sys.state.borrow_mut().procs.get_mut(&200).unwrap().alive = false;
    assert!(e.revert(tx).unwrap().ok());
}

#[test]
fn pid_reuse_is_detected() {
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("t").unwrap();
    let e = Engine::new(&j, &sys);
    let tx = e
        .apply(TxKind::Session, None, "s", &full_plan()[7..8])
        .unwrap();
    // GoogleUpdate exits and its PID is reused by an unrelated process.
    sys.state.borrow_mut().procs.insert(
        200,
        FakeProc {
            start_time: 9999,
            image: "notepad.exe".into(),
            priority: PriorityClass::Normal,
            eco_qos: false,
            suspended: false,
            alive: true,
        },
    );
    assert!(e.revert(tx).unwrap().ok(), "treated as gone");
    assert!(
        !sys.snapshot().procs[&200].suspended,
        "the unrelated process was not touched"
    );
}

// ---------------------------------------------------------------- safety

#[test]
fn kernel_anticheat_game_is_never_opened() {
    let sys = FakeSystem::new(original());
    let j = Journal::open_in_memory("t").unwrap();
    let mut policy = SafetyPolicy {
        game: Some((p(300, "game.exe"), AntiCheat::Kernel(AcKind::EasyAntiCheat))),
        ..Default::default()
    };
    policy.game_children.insert(301);
    let e = Engine::new(&j, &sys).with_guard(&policy);

    let bad = vec![
        PlannedAction::new("gamedvr_off", full_plan()[0].action.clone()),
        PlannedAction::new(
            "game_priority",
            Action::ProcessPriority {
                process: p(300, "game.exe"),
                class: PriorityClass::High,
            },
        ),
    ];
    let err = e.apply(TxKind::Session, None, "s", &bad).unwrap_err();
    assert!(matches!(err, TxError::Forbidden { pid: 300, .. }), "{err}");
    assert!(
        !sys.opened.borrow().contains(&300),
        "no handle to the protected game, ever"
    );
    assert_eq!(
        sys.snapshot(),
        original(),
        "refused before changing anything"
    );
    assert!(
        j.txs_in(&[TxState::Open, TxState::Applying, TxState::Applied])
            .unwrap()
            .is_empty(),
        "nothing journaled"
    );

    // filter_plan removes it upfront; the rest goes through.
    let (ok, blocked) = policy.filter_plan(bad.iter().map(|pa| pa.action.clone()).collect());
    assert_eq!((ok.len(), blocked.len()), (1, 1));
    assert_eq!(blocked[0].1, NeverTouch::AntiCheat);

    // Anti-cheat processes themselves are untouchable too.
    let ac = vec![PlannedAction::new(
        "x",
        Action::ProcessEcoQos {
            process: p(400, "EasyAntiCheat_EOS.exe"),
            on: true,
        },
    )];
    assert!(matches!(
        e.apply(TxKind::Session, None, "s", &ac),
        Err(TxError::Forbidden { .. })
    ));
    assert!(!sys.opened.borrow().contains(&400));
}

#[test]
fn a_game_without_anticheat_may_get_priority() {
    let mut policy = SafetyPolicy {
        game: Some((p(300, "game.exe"), AntiCheat::None)),
        ..Default::default()
    };
    assert!(policy.check_process(&p(300, "game.exe")).is_ok());
    policy.game = Some((p(300, "game.exe"), AntiCheat::Unknown));
    assert_eq!(
        policy.check_process(&p(300, "game.exe")),
        Err(NeverTouch::AntiCheat),
        "unknown = stay out"
    );
}

#[test]
fn anticheat_detection() {
    let g = |exe: &str, entries: &[&str]| {
        anticheat_status(&GameInfo {
            exe: exe.into(),
            install_entries: entries.iter().map(|s| s.to_string()).collect(),
        })
    };
    assert_eq!(
        g("VALORANT-Win64-Shipping.exe", &[]),
        AntiCheat::Kernel(AcKind::Vanguard)
    );
    assert_eq!(g("cs2.exe", &[]), AntiCheat::UserMode(AcKind::Vac));
    assert_eq!(
        g("SomeGame.exe", &["EasyAntiCheat", "SomeGame_Data"]),
        AntiCheat::Kernel(AcKind::EasyAntiCheat)
    );
    assert_eq!(
        g("Other.exe", &["BattlEye", "pnkbstra.exe"]),
        AntiCheat::Kernel(AcKind::BattlEye),
        "kernel signature wins"
    );
    assert_eq!(
        g("Indie.exe", &["Indie_Data", "UnityPlayer.dll"]),
        AntiCheat::None
    );
    assert_eq!(
        g("Mystery.exe", &[]),
        AntiCheat::Unknown,
        "didn't look → don't claim none"
    );
}

#[test]
fn background_policy_touches_only_known_safe_processes() {
    let mut policy = SafetyPolicy {
        game: Some((p(300, "game.exe"), AntiCheat::None)),
        ..Default::default()
    };
    policy.user_whitelist.insert("spotify.exe".into());
    let info = |pid: u32, img: &str, session: u32, fg: bool, audio: bool, window: bool| ProcInfo {
        process: p(pid, img),
        session_id: session,
        parent_pid: None,
        is_foreground: fg,
        produces_audio: audio,
        has_visible_window: window,
    };
    use BackgroundDecision::*;
    let cases = [
        (info(1, "chrome.exe", 1, false, false, true), LowerPriority),
        (
            info(2, "chrome.exe", 1, false, true, true),
            NeverTouch(NT::ProducingAudio),
        ),
        (
            info(3, "chrome.exe", 1, true, false, true),
            NeverTouch(NT::Foreground),
        ),
        (info(4, "GoogleUpdate.exe", 1, false, false, false), Suspend),
        (
            info(5, "Discord.exe", 1, false, false, true),
            NeverTouch(NT::OverlayOrVoice),
        ),
        (
            info(6, "steam.exe", 1, false, false, false),
            NeverTouch(NT::Launcher),
        ),
        (
            info(7, "steamwebhelper.exe", 1, false, false, false),
            NeverTouch(NT::Launcher),
        ),
        (
            info(8, "MsMpEng.exe", 1, false, false, false),
            NeverTouch(NT::Security),
        ),
        (
            info(9, "svchost.exe", 0, false, false, false),
            NeverTouch(NT::SystemSession),
        ),
        (
            info(10, "explorer.exe", 1, false, false, true),
            NeverTouch(NT::SystemOrShell),
        ),
        (
            info(11, "lghub_agent.exe", 1, false, false, false),
            NeverTouch(NT::Peripheral),
        ),
        (
            info(12, "spotify.exe", 1, false, false, true),
            NeverTouch(NT::Whitelisted),
        ),
        (
            info(13, "random_tool.exe", 1, false, false, false),
            NeverTouch(NT::Unknown),
        ),
        (
            info(300, "game.exe", 1, true, true, true),
            NeverTouch(NT::Game),
        ),
        (
            info(14, "vgc.exe", 1, false, false, false),
            NeverTouch(NT::AntiCheat),
        ),
        (
            info(15, "overdrive-ui.exe", 1, false, false, true),
            NeverTouch(NT::Overdrive),
        ),
    ];
    for (pi, want) in &cases {
        assert_eq!(
            policy.background_decision(pi),
            *want,
            "{}",
            pi.process.image
        );
    }
    let acts = policy.background_actions(&cases.iter().map(|c| c.0.clone()).collect::<Vec<_>>());
    assert_eq!(
        acts.len(),
        3,
        "chrome (eco + prio) + GoogleUpdate (suspend) only: {acts:?}"
    );
}
