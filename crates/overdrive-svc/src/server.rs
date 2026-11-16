//! Pipe server: one acceptor thread, a reader and a writer thread per client, and the
//! core running on the calling thread (the core owns non-`Send` state, so it never moves).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use overdrive_ipc::{Pipe, Server};

use crate::core::{ConnId, Core, Out};
use crate::library::norm;

pub enum Msg {
    Connected(ConnId, Sender<String>, Pipe),
    Request(ConnId, String),
    Disconnected(ConnId),
    Stop,
}

/// Who may talk to the service.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClientPolicy {
    /// Installed service: the client must be an executable from the service's own folder
    /// and run in an interactive session.
    InstallDirOnly,
    /// Console/dev: anyone the pipe DACL lets in.
    Any,
}

/// More than this many frames per second from one client drops the connection.
const RATE_LIMIT: u32 = 200;

fn allowed(pipe: &Pipe, policy: ClientPolicy) -> bool {
    if policy == ClientPolicy::Any {
        return true;
    }
    let Ok(pid) = pipe.peer_pid() else { return false };
    if overdrive_ipc::session_of(pid).unwrap_or(0) == 0 {
        return false;
    }
    let (Some(client), Ok(me)) = (overdrive_win::processes::image_path(pid), std::env::current_exe()) else {
        return false;
    };
    let dir = |p: &str| norm(p).rsplit_once('\\').map(|(d, _)| d.to_string());
    dir(&client).is_some() && dir(&client) == dir(&me.display().to_string())
}

/// Start accepting clients; returns the inbox the core loop reads from.
pub fn listen(name: &str, sddl: &str, policy: ClientPolicy) -> std::io::Result<(Sender<Msg>, Receiver<Msg>)> {
    let srv = Server::bind(name, sddl)?;
    let (tx, rx) = mpsc::channel::<Msg>();
    let inbox = tx.clone();
    let next = Arc::new(AtomicU64::new(1));
    thread::spawn(move || loop {
        let Ok(pipe) = srv.accept() else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        if !allowed(&pipe, policy) {
            pipe.close();
            continue;
        }
        let id = next.fetch_add(1, Ordering::Relaxed);
        let (wtx, wrx) = mpsc::channel::<String>();
        let w = pipe.clone();
        thread::spawn(move || {
            for f in wrx {
                if w.write_frame(&f).is_err() {
                    w.close();
                    break;
                }
            }
        });
        // Register the connection BEFORE reading: its first request must find it.
        if inbox.send(Msg::Connected(id, wtx, pipe.clone())).is_err() {
            return;
        }
        let r = pipe.clone();
        let inbox_r = inbox.clone();
        thread::spawn(move || {
            let (mut window, mut count) = (Instant::now(), 0u32);
            loop {
                match r.read_frame() {
                    Ok(f) => {
                        if window.elapsed() >= Duration::from_secs(1) {
                            window = Instant::now();
                            count = 0;
                        }
                        count += 1;
                        if count > RATE_LIMIT || inbox_r.send(Msg::Request(id, f)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            r.close();
            let _ = inbox_r.send(Msg::Disconnected(id));
        });
    });
    Ok((tx, rx))
}

fn flush(core: &mut Core, conns: &HashMap<ConnId, (Sender<String>, Pipe)>) {
    for o in core.take_out() {
        match o {
            Out::Reply(c, f) => {
                if let Some((s, _)) = conns.get(&c) {
                    let _ = s.send(f);
                }
            }
            Out::Broadcast(f) => {
                for (s, _) in conns.values() {
                    let _ = s.send(f.clone());
                }
            }
        }
    }
}

/// The core loop. Returns when `Msg::Stop` arrives (after the core shut down cleanly).
pub fn run(mut core: Core, rx: Receiver<Msg>) {
    let mut conns: HashMap<ConnId, (Sender<String>, Pipe)> = HashMap::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Msg::Connected(id, s, p)) => {
                conns.insert(id, (s, p));
            }
            Ok(Msg::Request(id, f)) => {
                core.request(id, &f);
                flush(&mut core, &conns);
                core.run_queued();
            }
            Ok(Msg::Disconnected(id)) => {
                conns.remove(&id);
            }
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                core.shutdown();
                flush(&mut core, &conns);
                for (_, p) in conns.values() {
                    p.close();
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        core.tick();
        flush(&mut core, &conns);
    }
}
