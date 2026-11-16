//! The real pipe server + core loop: many clients in a row, each one answered.

use std::thread;
use std::time::Duration;

use overdrive_core::build_profile;
use overdrive_core::engine::fake::{FakeState, FakeSystem};
use overdrive_core::engine::RegRoot;
use overdrive_core::profile::HardwareProfile;
use overdrive_core::safety::ProcInfo;
use overdrive_core::source::RawStabilityEvents;
use overdrive_core::testing::FakeSource;
use overdrive_svc::core::{Config, Core};
use overdrive_svc::platform::{Capture, LibraryGame, Platform};
use overdrive_svc::server::{self, ClientPolicy, Msg};

struct Quiet;
impl Platform for Quiet {
    fn profile(&mut self, _: &RegRoot) -> HardwareProfile { build_profile(&FakeSource::desktop_entry()) }
    fn processes(&mut self) -> Vec<ProcInfo> { vec![] }
    fn image_path(&mut self, _: u32) -> Option<String> { None }
    fn library(&mut self, _: &RegRoot) -> Vec<LibraryGame> { vec![] }
    fn capture(&mut self, _: u32) -> Result<Box<dyn Capture>, String> { Err("none".into()) }
    fn user_root(&mut self) -> RegRoot { RegRoot::CurrentUser }
    fn user_dirs(&mut self, _: &RegRoot) -> overdrive_core::games::UserDirs { Default::default() }
    fn reboot_to_firmware(&mut self) -> Result<(), String> { Err("no".into()) }
    fn stability_events(&mut self, _: i64) -> Option<RawStabilityEvents> { None }
    fn now_ms(&self) -> i64 { 1_760_000_000_000 }
    fn local_hour(&self) -> u8 { 12 }
}

#[test]
fn every_client_gets_its_replies() {
    let name = format!(r"\\.\pipe\overdrive.svctest.{}", std::process::id());
    let dir = std::env::temp_dir().join(format!("overdrive-pipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let n2 = name.clone();
    let (stx, srx) = std::sync::mpsc::channel();
    let h = thread::spawn(move || {
        let core = Core::new(
            Config { data_dir: dir.clone(), svc_version: "t".into(), allow_reboot: false },
            Box::new(FakeSystem::new(FakeState::default())),
            Box::new(Quiet),
        )
        .unwrap();
        let (tx, rx) = server::listen(&n2, overdrive_ipc::DEFAULT_SDDL, ClientPolicy::Any).unwrap();
        stx.send(tx).unwrap();
        server::run(core, rx);
        let _ = std::fs::remove_dir_all(&dir);
    });
    let stop = srx.recv().unwrap();
    for i in 0..20 {
        let c = overdrive_ipc::connect(&name, Duration::from_secs(3)).unwrap();
        // Fire immediately after connecting: the race this test guards against.
        c.write_frame(r#"{"kind":"req","id":1,"method":"hello","params":{}}"#).unwrap();
        c.write_frame(r#"{"kind":"req","id":2,"method":"state.snapshot"}"#).unwrap();
        let mut got = vec![];
        while got.len() < 2 {
            let v: serde_json::Value = serde_json::from_str(&c.read_frame().unwrap()).unwrap();
            if v["kind"] == "res" {
                assert_eq!(v["ok"], true, "client {i}: {v}");
                got.push(v["id"].as_u64().unwrap());
            }
        }
        assert_eq!(got, vec![1, 2]);
    }
    stop.send(Msg::Stop).unwrap();
    h.join().unwrap();
}
