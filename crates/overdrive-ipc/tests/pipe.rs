//! Real named pipes, in-process: framing, concurrency, limits, disconnects.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use overdrive_ipc::{connect, session_of, Server, DEFAULT_SDDL, MAX_FRAME};

fn name(tag: &str) -> String {
    format!(r"\\.\pipe\overdrive.test.{tag}.{}", std::process::id())
}

#[test]
fn round_trip_and_peer_identity() {
    let n = name("rt");
    let srv = Server::bind(&n, DEFAULT_SDDL).unwrap();
    let h = thread::spawn(move || {
        let c = srv.accept().unwrap();
        assert_eq!(c.peer_pid().unwrap(), std::process::id());
        let f = c.read_frame().unwrap();
        c.write_frame(&format!("echo:{f}")).unwrap();
        // Unicode and an empty frame survive.
        c.write_frame("¿Mantener siempre? ✓").unwrap();
        c.write_frame("").unwrap();
    });
    let cl = connect(&n, Duration::from_secs(2)).unwrap();
    assert_eq!(cl.peer_pid().unwrap(), std::process::id());
    cl.write_frame(r#"{"kind":"req","id":1,"method":"hello"}"#).unwrap();
    assert_eq!(
        cl.read_frame().unwrap(),
        r#"echo:{"kind":"req","id":1,"method":"hello"}"#
    );
    assert_eq!(cl.read_frame().unwrap(), "¿Mantener siempre? ✓");
    assert_eq!(cl.read_frame().unwrap(), "");
    h.join().unwrap();
    assert!(session_of(std::process::id()).is_some());
}

#[test]
fn write_is_not_blocked_by_a_pending_read() {
    // The reason for overlapped I/O: the server pushes events while the client's reader
    // thread is parked in ReadFile on the same handle.
    let n = name("concurrent");
    let srv = Server::bind(&n, DEFAULT_SDDL).unwrap();
    let (tx, rx) = mpsc::channel();
    let h = thread::spawn(move || {
        let c = srv.accept().unwrap();
        let r = c.clone();
        let reader = thread::spawn(move || r.read_frame().unwrap());
        thread::sleep(Duration::from_millis(150)); // reader is now blocked
        for i in 0..50 {
            c.write_frame(&format!("evt{i}")).unwrap();
        }
        tx.send(()).unwrap();
        assert_eq!(reader.join().unwrap(), "late request");
    });
    let cl = connect(&n, Duration::from_secs(2)).unwrap();
    for i in 0..50 {
        assert_eq!(cl.read_frame().unwrap(), format!("evt{i}"));
    }
    rx.recv_timeout(Duration::from_secs(2)).expect("writer never blocked");
    cl.write_frame("late request").unwrap();
    h.join().unwrap();
}

#[test]
fn oversized_frames_are_refused_both_ways() {
    let n = name("big");
    let srv = Server::bind(&n, DEFAULT_SDDL).unwrap();
    let h = thread::spawn(move || {
        let c = srv.accept().unwrap();
        let e = c.read_frame().unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        c.close();
    });
    let cl = connect(&n, Duration::from_secs(2)).unwrap();
    let big = "x".repeat(MAX_FRAME + 1);
    assert!(cl.write_frame(&big).is_err(), "sender refuses too");
    // A hostile client lies in the length prefix.
    cl.write_raw(&((MAX_FRAME as u32) + 1).to_le_bytes()).unwrap();
    h.join().unwrap();
    assert!(cl.read_frame().is_err(), "server dropped the connection");
}

#[test]
fn disconnect_unblocks_the_reader_and_name_stays_owned() {
    let n = name("dc");
    let srv = Server::bind(&n, DEFAULT_SDDL).unwrap();
    // Name already owned → a second server (a squatter) cannot bind it.
    assert!(Server::bind(&n, DEFAULT_SDDL).is_err());
    let h = thread::spawn(move || {
        let c = srv.accept().unwrap();
        assert_eq!(c.read_frame().unwrap(), "first");
        c.write_frame("ack").unwrap();
        let e = c.read_frame().unwrap_err();
        assert!(matches!(
            e.kind(),
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
        ));
        // Next client still finds a listening instance.
        let c2 = srv.accept().unwrap();
        c2.write_frame("second").unwrap();
    });
    let cl = connect(&n, Duration::from_secs(2)).unwrap();
    cl.write_frame("first").unwrap();
    assert_eq!(cl.read_frame().unwrap(), "ack"); // served for sure
    drop(cl);
    let cl2 = connect(&n, Duration::from_secs(2)).unwrap();
    assert_eq!(cl2.read_frame().unwrap(), "second");
    h.join().unwrap();
}

#[test]
fn client_that_leaves_before_accept_is_skipped() {
    let n = name("early");
    let srv = Server::bind(&n, DEFAULT_SDDL).unwrap();
    drop(connect(&n, Duration::from_secs(2)).unwrap()); // gone before accept()
    let h = thread::spawn(move || {
        let c = srv.accept().unwrap();
        c.write_frame("live").unwrap();
    });
    let cl = connect(&n, Duration::from_secs(2)).unwrap();
    assert_eq!(cl.read_frame().unwrap(), "live");
    h.join().unwrap();
}

#[test]
fn close_from_another_thread_unblocks_read() {
    let n = name("close");
    let srv = Server::bind(&n, DEFAULT_SDDL).unwrap();
    let h = thread::spawn(move || srv.accept().unwrap());
    let cl = connect(&n, Duration::from_secs(2)).unwrap();
    let _server_end = h.join().unwrap();
    let r = cl.clone();
    let reader = thread::spawn(move || r.read_frame());
    thread::sleep(Duration::from_millis(100));
    cl.close();
    assert!(reader.join().unwrap().is_err());
}

#[test]
fn missing_service_is_not_found() {
    let e = connect(&name("nobody"), Duration::from_millis(100)).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
}
