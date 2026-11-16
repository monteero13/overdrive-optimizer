//! Phase 5 against the REAL system (Windows registry + a real process).
//!
//! `real_process_killed_at_every_point` is the acceptance test from the brief: a child
//! process applies (or reverts) real registry changes and is hard-killed (`abort()`) at
//! every crash point; a fresh process then runs recovery and the registry must be identical
//! to the original. Everything happens under HKCU\Software\OverdriveTest\<unique> and is
//! deleted afterwards — nothing outside that sandbox is modified.

use std::cell::Cell;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use overdrive_core::engine::journal::{Journal, TxKind};
use overdrive_core::engine::tx::{CrashPoint, Engine, PlannedAction};
use overdrive_core::engine::{
    Action, PriorityClass, ProcRef, RegData, RegRoot, RegTarget, StartType, SystemOps,
};
use overdrive_win::system_ops::WinSystem;

const HKCU: RegRoot = RegRoot::CurrentUser;
const PARENT: &str = r"Software\OverdriveTest";

fn sandbox(tag: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(r"{PARENT}\{tag}-{}-{n}", std::process::id())
}

fn seed(sb: &str) {
    let s = WinSystem;
    s.delete_tree(&HKCU, sb).unwrap();
    s.reg_write(&HKCU, sb, "Existing", &RegData::Dword(1))
        .unwrap();
    s.reg_write(&HKCU, sb, "Text", &RegData::String("hello".into()))
        .unwrap();
    s.reg_write(&HKCU, sb, "ToDelete", &RegData::String("bye".into()))
        .unwrap();
}

#[derive(Debug, PartialEq)]
struct Snap {
    existing: Option<RegData>,
    text: Option<RegData>,
    to_delete: Option<RegData>,
    deep_value: Option<RegData>,
    deep_key: bool,
    deeper_key: bool,
}

fn snap(sb: &str) -> Snap {
    let s = WinSystem;
    Snap {
        existing: s.reg_read(&HKCU, sb, "Existing").unwrap(),
        text: s.reg_read(&HKCU, sb, "Text").unwrap(),
        to_delete: s.reg_read(&HKCU, sb, "ToDelete").unwrap(),
        deep_value: s
            .reg_read(&HKCU, &format!("{sb}\\Deep\\Er"), "New")
            .unwrap(),
        deep_key: s.reg_key_exists(&HKCU, &format!("{sb}\\Deep")).unwrap(),
        deeper_key: s.reg_key_exists(&HKCU, &format!("{sb}\\Deep\\Er")).unwrap(),
    }
}

fn plan(sb: &str) -> Vec<PlannedAction> {
    let rv = |name: &str, key: String, target| Action::RegistryValue {
        root: HKCU,
        key,
        name: name.into(),
        target,
    };
    vec![
        PlannedAction::new(
            "set_existing",
            rv("Existing", sb.into(), RegTarget::Set(RegData::Dword(0))),
        ),
        PlannedAction::new(
            "create_deep",
            rv(
                "New",
                format!("{sb}\\Deep\\Er"),
                RegTarget::Set(RegData::String("GpuPreference=2;".into())),
            ),
        ),
        PlannedAction::new("delete_value", rv("ToDelete", sb.into(), RegTarget::Delete)),
        PlannedAction::new(
            "set_text",
            rv(
                "Text",
                sb.into(),
                RegTarget::Set(RegData::String("changed".into())),
            ),
        ),
    ]
}

fn db_path(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("overdrive-live-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("journal.db")
}

#[test]
fn registry_cycle_on_the_real_registry() {
    let sb = sandbox("cycle");
    seed(&sb);
    let original = snap(&sb);
    assert!(!original.deep_key);

    let j = Journal::open_in_memory("live").unwrap();
    let sys = WinSystem;
    let e = Engine::new(&j, &sys);
    let tx = e.apply(TxKind::Session, None, "live", &plan(&sb)).unwrap();
    let applied = snap(&sb);
    assert_eq!(applied.existing, Some(RegData::Dword(0)));
    assert_eq!(applied.to_delete, None);
    assert!(
        applied.deep_key && applied.deeper_key,
        "apply created the key chain"
    );

    let r = e.revert(tx).unwrap();
    assert!(r.ok(), "{r:?}");
    assert_eq!(
        snap(&sb),
        original,
        "registry identical after revert (created keys removed too)"
    );
    cleanup(&sb);
}

/// Remove the sandbox, and the shared parent key if no other test is still using it.
fn cleanup(sb: &str) {
    WinSystem.delete_tree(&HKCU, sb).unwrap();
    WinSystem.reg_delete_key_if_empty(&HKCU, PARENT).unwrap();
}

/// Child entry point: only acts when spawned by `real_process_killed_at_every_point`.
#[test]
fn kill_child() {
    let (Ok(db), Ok(sb), Ok(n), Ok(mode)) = (
        std::env::var("OVD_KILL_DB"),
        std::env::var("OVD_KILL_KEY"),
        std::env::var("OVD_KILL_N"),
        std::env::var("OVD_KILL_MODE"),
    ) else {
        return;
    };
    let n: usize = n.parse().unwrap();
    let j = Journal::open(std::path::Path::new(&db), "live-child").unwrap();
    let sys = WinSystem;
    let count = Cell::new(0usize);
    let killer = |_: CrashPoint| {
        if count.get() == n {
            std::process::abort(); // a real, unclean process death
        }
        count.set(count.get() + 1);
    };
    if mode == "apply" {
        let mut e = Engine::new(&j, &sys);
        e.hook = Some(Box::new(killer));
        let _ = e.apply(TxKind::Session, None, "live", &plan(&sb));
    } else {
        let tx = Engine::new(&j, &sys)
            .apply(TxKind::Session, None, "live", &plan(&sb))
            .unwrap();
        let mut e = Engine::new(&j, &sys);
        e.hook = Some(Box::new(killer));
        let _ = e.revert(tx);
    }
    // Reaching here means the crash point wasn't hit: exit cleanly so the parent notices.
}

#[test]
fn real_process_killed_at_every_point() {
    let sb = sandbox("kill");
    seed(&sb);
    let original = snap(&sb);

    // How many crash points do apply and revert pass through?
    let (mut apply_points, mut revert_points) = (0usize, 0usize);
    {
        let j = Journal::open_in_memory("count").unwrap();
        let sys = WinSystem;
        let c = Cell::new(0usize);
        let mut e = Engine::new(&j, &sys);
        e.hook = Some(Box::new(|_| c.set(c.get() + 1)));
        let tx = e.apply(TxKind::Session, None, "count", &plan(&sb)).unwrap();
        apply_points = apply_points.max(c.get());
        c.set(0);
        e.revert(tx).unwrap();
        revert_points = revert_points.max(c.get());
    }
    assert_eq!(snap(&sb), original);
    assert_eq!(apply_points, 2 + 4 * 4);
    assert_eq!(revert_points, 4 * 2);

    let exe = std::env::current_exe().unwrap();
    let mut checked = 0;
    for (mode, points) in [("apply", apply_points), ("revert", revert_points)] {
        for n in 0..points {
            seed(&sb);
            let db = db_path(&format!("{mode}-{n}"));
            let status = Command::new(&exe)
                .args(["kill_child", "--exact", "--nocapture", "--test-threads=1"])
                .env("OVD_KILL_DB", &db)
                .env("OVD_KILL_KEY", &sb)
                .env("OVD_KILL_N", n.to_string())
                .env("OVD_KILL_MODE", mode)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .status()
                .unwrap();
            assert!(
                !status.success(),
                "{mode} #{n}: child should have been killed, exited with {status}"
            );

            // "The service restarts": a fresh process opens the journal and recovers.
            let j = Journal::open(&db, "live-recover").unwrap();
            let rep = Engine::new(&j, &WinSystem).recover().unwrap();
            assert!(rep.reverted.iter().all(|r| r.ok()), "{mode} #{n}: {rep:?}");
            assert_eq!(
                snap(&sb),
                original,
                "{mode} killed at point #{n}: registry not restored"
            );
            checked += 1;
            drop(j);
            let _ = std::fs::remove_dir_all(db.parent().unwrap());
        }
    }
    println!("real kill test: {checked} hard kills, registry restored every time");
    cleanup(&sb);
}

#[test]
fn real_process_priority_ecoqos_suspend_and_resume() {
    // A harmless process we own: ping localhost for a minute.
    let mut child = Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdout(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    let proc = WinSystem::proc_ref(child.id()).unwrap();
    assert_eq!(proc.image.to_ascii_lowercase(), "ping.exe");
    let sys = WinSystem;
    // The child inherits our priority (CI runners use BelowNormal): work from what it has.
    let original = sys.proc_priority(&proc).unwrap();
    let target = if original == PriorityClass::BelowNormal { PriorityClass::Idle } else { PriorityClass::BelowNormal };
    let eco_before = sys.proc_eco_qos(&proc).unwrap();

    let j = Journal::open_in_memory("live").unwrap();
    let e = Engine::new(&j, &sys);
    let plan = vec![
        PlannedAction::new(
            "bg_eco",
            Action::ProcessEcoQos {
                process: proc.clone(),
                on: true,
            },
        ),
        PlannedAction::new(
            "bg_prio",
            Action::ProcessPriority {
                process: proc.clone(),
                class: target,
            },
        ),
        PlannedAction::new(
            "bg_suspend",
            Action::ProcessSuspend {
                process: proc.clone(),
            },
        ),
    ];
    let tx = e.apply(TxKind::Session, None, "live", &plan).unwrap();
    assert_eq!(
        sys.proc_priority(&proc).unwrap(),
        target
    );
    assert_ne!(
        sys.proc_eco_qos(&proc).unwrap(),
        Some(false),
        "EcoQoS on (or not queryable on this Windows build)"
    );

    let r = e.revert(tx).unwrap();
    assert!(r.ok(), "{r:?}");
    assert_eq!(sys.proc_priority(&proc).unwrap(), original);
    assert_eq!(sys.proc_eco_qos(&proc).unwrap(), eco_before);
    assert!(sys.proc_alive(&proc).unwrap(), "resumed and still running");

    // PID-reuse protection: same PID, different creation time → never touched.
    let impostor = ProcRef {
        start_time: proc.start_time + 1,
        ..proc.clone()
    };
    assert!(!sys.proc_alive(&impostor).unwrap());
    assert!(sys
        .set_proc_priority(&impostor, PriorityClass::Idle)
        .is_err());
    assert_eq!(
        sys.proc_priority(&proc).unwrap(),
        original,
        "the real process was not touched"
    );

    let _ = child.kill();
    let _ = child.wait();
    assert!(!sys.proc_alive(&proc).unwrap(), "exited");
}

#[test]
fn read_only_system_queries_work() {
    let sys = WinSystem;
    assert!(matches!(
        sys.service_start_type("EventLog").unwrap(),
        StartType::Automatic
    ));
    assert!(sys.service_start_type("Overdrive_No_Such_Service").is_err());
    let g = sys.active_power_scheme().unwrap();
    assert_eq!(g.len(), 36, "{g}");
    assert!(sys.reg_key_exists(&HKCU, "Software").unwrap());
    assert!(!sys
        .reg_key_exists(&HKCU, "Software\\Overdrive_No_Such_Key")
        .unwrap());
}
