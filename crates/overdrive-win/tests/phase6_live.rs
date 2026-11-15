//! Phase 6 primitives against the real system — read-only (no change is made).

use overdrive_core::engine::{SysError, SystemOps};
use overdrive_core::safety::{BackgroundDecision, NeverTouch, SafetyPolicy};
use overdrive_win::processes;
use overdrive_win::system_ops::WinSystem;

#[test]
fn power_schemes_include_the_active_one() {
    let s = WinSystem;
    let all = s.power_schemes().unwrap();
    let active = s.active_power_scheme().unwrap();
    println!("power schemes: {all:?} (active {active})");
    assert!(!all.is_empty());
    assert!(all.contains(&active), "active scheme must be listed");
    assert!(all
        .iter()
        .all(|g| g.len() == 36 && g == &g.to_ascii_lowercase()));
}

#[test]
fn task_scheduler_reads_real_tasks() {
    let s = WinSystem;
    // Present on every Windows 10/11 install.
    let enabled = s
        .task_enabled(r"\Microsoft\Windows\Defrag\ScheduledDefrag")
        .unwrap();
    println!("ScheduledDefrag enabled: {enabled}");
    assert!(matches!(
        s.task_enabled(r"\Microsoft\Windows\Overdrive\NoSuchTask"),
        Err(SysError::NotFound(_))
    ));
    // The telemetry tasks the catalog may disable: report which exist on this build.
    for t in overdrive_core::catalog::TELEMETRY_TASKS {
        println!("  {t}: {:?}", s.task_enabled(t));
    }
}

#[test]
fn process_list_has_identity_session_and_audio() {
    let me = std::process::id();
    let list = processes::list().unwrap();
    assert!(list.len() > 20, "only {} processes", list.len());
    let mine = list
        .iter()
        .find(|p| p.process.pid == me)
        .expect("own process listed");
    assert!(mine.session_id > 0, "we run in a user session");
    assert!(mine.process.start_time > 0);
    let audio = processes::audio_pids();
    let windows = processes::windowed_pids();
    println!(
        "{} processes, {} with active audio, {} with visible windows, foreground {:?}",
        list.len(),
        audio.len(),
        windows.len(),
        processes::foreground_pid()
    );

    // Run the real policy over the real process list: what WOULD be touched (nothing is).
    let policy = SafetyPolicy::default();
    let would = policy.background_actions(&list);
    let audio_images: std::collections::HashSet<String> = list
        .iter()
        .filter(|p| audio.contains(&p.process.pid))
        .map(|p| p.process.image.to_ascii_lowercase())
        .collect();
    for a in &would {
        let img = a.process().unwrap().image.to_ascii_lowercase();
        assert!(
            !audio_images.contains(&img),
            "{img} has a process playing audio: none of its processes may be touched"
        );
    }
    let mut lowered = vec![];
    let mut suspended = vec![];
    for p in &list {
        match policy.background_decision(p) {
            BackgroundDecision::LowerPriority => lowered.push(p.process.image.clone()),
            BackgroundDecision::Suspend => suspended.push(p.process.image.clone()),
            BackgroundDecision::NeverTouch(_) => {}
        }
        if audio.contains(&p.process.pid) {
            assert!(
                matches!(
                    policy.background_decision(p),
                    BackgroundDecision::NeverTouch(_)
                ),
                "{} produces audio and must never be touched",
                p.process.image
            );
        }
        if p.process.image.eq_ignore_ascii_case("explorer.exe") {
            assert_eq!(
                policy.background_decision(p),
                BackgroundDecision::NeverTouch(NeverTouch::SystemOrShell)
            );
        }
    }
    lowered.sort();
    lowered.dedup();
    println!("policy on this PC → would lower: {lowered:?}; would suspend: {suspended:?}");

    let tree = processes::descendants(&list, me);
    assert!(tree.contains(&me));
}
