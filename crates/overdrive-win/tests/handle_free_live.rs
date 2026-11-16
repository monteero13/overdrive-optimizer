//! Live, read-only: the handle-free process list matches reality.

use overdrive_win::{identity, processes};

#[test]
fn snapshot_sees_itself_without_handles() {
    let me = std::process::id();
    let snap = processes::snapshot().expect("snapshot");
    assert!(snap.len() > 20, "only {} processes", snap.len());
    let mine = snap.iter().find(|p| p.pid == me).expect("own process listed");
    assert!(mine.start_time > 0);
    assert!(mine.image.to_ascii_lowercase().ends_with(".exe"));
    // Parent chain is real.
    assert!(snap.iter().any(|p| p.pid == mine.parent_pid));
    // Full path via SystemProcessIdInformation equals current_exe().
    let path = processes::image_path(me).expect("image path");
    let exe = std::env::current_exe().unwrap();
    assert_eq!(
        path.to_ascii_lowercase(),
        exe.display().to_string().to_ascii_lowercase()
    );
    // list() keeps the same identities.
    let list = processes::list().unwrap();
    let li = list.iter().find(|p| p.process.pid == me).unwrap();
    assert_eq!(li.process.start_time, mine.start_time);
}

#[test]
fn identity_reads_current_sid() {
    let sid = identity::current_user_sid().expect("sid");
    assert!(sid.starts_with("S-1-5-"), "{sid}");
    assert!(identity::console_session_id().is_some());
    if !identity::is_local_system() {
        // Only LocalSystem may query another session's token.
        assert_eq!(identity::console_user_sid(), None);
    }
}
