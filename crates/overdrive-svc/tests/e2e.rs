//! The whole service, driven like the real UI would drive it, on a fake machine:
//! library → baseline sessions → "Optimize" → A/B sessions → report → "keep it" →
//! revert one → revert all → crash recovery. The simulated game's performance depends only
//! on what is REALLY applied in the fake system while it runs.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use overdrive_core::build_profile;
use overdrive_core::catalog::{GPU_PREFS_KEY, HIGH_PERFORMANCE};
use overdrive_core::engine::fake::{FakeProc, FakeState, FakeSystem};
use overdrive_core::engine::*;
use overdrive_core::profile::HardwareProfile;
use overdrive_core::safety::ProcInfo;
use overdrive_core::source::RawStabilityEvents;
use overdrive_core::synth::Rng;
use overdrive_core::telemetry::{FrameSample, SystemSample};
use overdrive_core::testing::FakeSource;
use overdrive_svc::core::{Config, Core, Out};
use overdrive_svc::platform::{Capture, LibraryGame, Platform};
use serde_json::{json, Value};

const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const DIR: &str = r"C:\Games\Hollow";
const EXE: &str = r"C:\Games\Hollow\hollow_knight.exe";
const GAME_PID: u32 = 300;

// ------------------------------------------------------------------ shared fake system

#[derive(Clone)]
struct Shared(Rc<FakeSystem>);

impl SystemOps for Shared {
    fn reg_read(&self, r: &RegRoot, k: &str, n: &str) -> SysResult<Option<RegData>> { self.0.reg_read(r, k, n) }
    fn reg_write(&self, r: &RegRoot, k: &str, n: &str, d: &RegData) -> SysResult<()> { self.0.reg_write(r, k, n, d) }
    fn reg_delete(&self, r: &RegRoot, k: &str, n: &str) -> SysResult<()> { self.0.reg_delete(r, k, n) }
    fn reg_key_exists(&self, r: &RegRoot, k: &str) -> SysResult<bool> { self.0.reg_key_exists(r, k) }
    fn reg_delete_key_if_empty(&self, r: &RegRoot, k: &str) -> SysResult<()> { self.0.reg_delete_key_if_empty(r, k) }
    fn service_start_type(&self, n: &str) -> SysResult<StartType> { self.0.service_start_type(n) }
    fn set_service_start_type(&self, n: &str, s: StartType) -> SysResult<()> { self.0.set_service_start_type(n, s) }
    fn active_power_scheme(&self) -> SysResult<String> { self.0.active_power_scheme() }
    fn set_power_scheme(&self, g: &str) -> SysResult<()> { self.0.set_power_scheme(g) }
    fn power_schemes(&self) -> SysResult<Vec<String>> { self.0.power_schemes() }
    fn task_enabled(&self, p: &str) -> SysResult<bool> { self.0.task_enabled(p) }
    fn set_task_enabled(&self, p: &str, e: bool) -> SysResult<()> { self.0.set_task_enabled(p, e) }
    fn proc_alive(&self, p: &ProcRef) -> SysResult<bool> { self.0.proc_alive(p) }
    fn proc_priority(&self, p: &ProcRef) -> SysResult<PriorityClass> { self.0.proc_priority(p) }
    fn set_proc_priority(&self, p: &ProcRef, c: PriorityClass) -> SysResult<()> { self.0.set_proc_priority(p, c) }
    fn proc_eco_qos(&self, p: &ProcRef) -> SysResult<Option<bool>> { self.0.proc_eco_qos(p) }
    fn set_proc_eco_qos(&self, p: &ProcRef, on: bool) -> SysResult<()> { self.0.set_proc_eco_qos(p, on) }
    fn suspend(&self, p: &ProcRef) -> SysResult<()> { self.0.suspend(p) }
    fn resume(&self, p: &ProcRef) -> SysResult<()> { self.0.resume(p) }
}

fn fproc(pid: u32, image: &str) -> (u32, FakeProc) {
    (pid, FakeProc { start_time: 1000 + pid as u64, image: image.into(), priority: PriorityClass::Normal, eco_qos: false, suspended: false, alive: true })
}

fn original() -> FakeState {
    let mut s = FakeState { power: BALANCED.into(), ..Default::default() };
    s.power_schemes.insert(HIGH_PERFORMANCE.into());
    s.add_reg(&RegRoot::CurrentUser, r"System\GameConfigStore", "GameDVR_Enabled", RegData::Dword(1));
    s.add_reg(&RegRoot::LocalMachine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuild", RegData::String("26100".into()));
    s.services.insert("DiagTrack".into(), StartType::Automatic);
    s.procs.extend([fproc(100, "chrome.exe"), fproc(200, "GoogleUpdate.exe"), fproc(GAME_PID, "hollow_knight.exe"), fproc(400, "Discord.exe")]);
    s
}

/// What is really in effect → hidden % gain of the 1 % low.
fn true_gain(s: &FakeState) -> f64 {
    let mut g = 0.0;
    let reg = |root: &RegRoot, k: &str, n: &str| s.reg.get(&(format!("{root:?}"), k.to_ascii_lowercase(), n.to_ascii_lowercase())).cloned();
    if matches!(reg(&RegRoot::CurrentUser, GPU_PREFS_KEY, EXE), Some(RegData::String(v)) if v.contains("GpuPreference=2")) {
        g += 30.0;
    }
    if s.power == HIGH_PERFORMANCE {
        g += 6.0;
    }
    if reg(&RegRoot::CurrentUser, r"System\GameConfigStore", "GameDVR_Enabled") == Some(RegData::Dword(0)) {
        g += 5.0;
    }
    if s.procs[&100].eco_qos {
        g -= 8.0;
    }
    g
}

// ------------------------------------------------------------------ fake platform

struct World {
    clock: Cell<i64>,
    game_running: Cell<bool>,
    sys: Rc<FakeSystem>,
    rng: RefCell<Rng>,
    captures: Cell<usize>,
}

struct FakePlatform(Rc<World>, HardwareProfile);

struct FakeCapture(Vec<FrameSample>);
impl Capture for FakeCapture {
    fn poll(&mut self) -> Vec<FrameSample> {
        std::mem::take(&mut self.0)
    }
    fn finish(self: Box<Self>) -> Vec<SystemSample> {
        vec![SystemSample { t_ms: 0.0, on_battery: Some(false), ..Default::default() }]
    }
}

fn frames(rng: &mut Rng, gain: f64) -> Vec<FrameSample> {
    let noise = 1.0 + rng.sym() * 0.035;
    let base = 16.7 / (1.0 + gain / 100.0) * noise;
    let n = (70_000.0 / base) as usize;
    let mut t = 0.0;
    (0..n)
        .map(|i| {
            let mut ft = base * (1.0 + rng.sym() * 0.06);
            if i % 700 == 350 {
                ft = base * 3.0 + 12.0;
            }
            let f = FrameSample { t_ms: t, frametime_ms: ft, gpu_busy_ms: Some(ft * 0.92), display_latency_ms: None };
            t += ft;
            f
        })
        .collect()
}

impl Platform for FakePlatform {
    fn profile(&mut self, _: &RegRoot) -> HardwareProfile {
        self.1.clone()
    }
    fn processes(&mut self) -> Vec<ProcInfo> {
        let p = |pid: u32, img: &str, win: bool, audio: bool| ProcInfo {
            process: ProcRef { pid, start_time: 1000 + pid as u64, image: img.into() },
            session_id: 1,
            parent_pid: None,
            is_foreground: pid == GAME_PID,
            produces_audio: audio,
            has_visible_window: win,
        };
        let mut v = vec![p(100, "chrome.exe", true, false), p(200, "GoogleUpdate.exe", false, false), p(400, "Discord.exe", true, true)];
        if self.0.game_running.get() {
            v.push(p(GAME_PID, "hollow_knight.exe", true, false));
        }
        v
    }
    fn image_path(&mut self, pid: u32) -> Option<String> {
        match pid {
            GAME_PID => Some(EXE.into()),
            100 => Some(r"C:\Program Files\Google\Chrome\chrome.exe".into()),
            400 => Some(r"C:\Users\u\AppData\Local\Discord\Discord.exe".into()),
            _ => None,
        }
    }
    fn library(&mut self, _: &RegRoot) -> Vec<LibraryGame> {
        vec![LibraryGame {
            id: "steam:367520".into(),
            name: "Hollow Knight".into(),
            launcher: "steam",
            install_dir: DIR.into(),
            exe_hint: None,
            install_entries: vec!["hollow_knight.exe".into(), "hollow_knight_data".into()],
        }]
    }
    fn capture(&mut self, _pid: u32) -> Result<Box<dyn Capture>, String> {
        self.0.captures.set(self.0.captures.get() + 1);
        let gain = true_gain(&self.0.sys.snapshot());
        Ok(Box::new(FakeCapture(frames(&mut self.0.rng.borrow_mut(), gain))))
    }
    fn user_root(&mut self) -> RegRoot {
        RegRoot::CurrentUser
    }
    fn reboot_to_firmware(&mut self) -> Result<(), String> {
        panic!("never in tests")
    }
    fn stability_events(&mut self, _: i64) -> Option<RawStabilityEvents> {
        None
    }
    fn now_ms(&self) -> i64 {
        self.0.clock.get()
    }
    fn local_hour(&self) -> u8 {
        20
    }
}

// ------------------------------------------------------------------ driver

struct Ui {
    core: Core,
    world: Rc<World>,
    next_id: u64,
    seq: u64,
    events: Vec<Value>,
}

impl Ui {
    fn new(dir: &PathBuf, world: Rc<World>, sys: Rc<FakeSystem>) -> Self {
        let hw = build_profile(&FakeSource::laptop_hybrid());
        let core = Core::new(
            Config { data_dir: dir.clone(), svc_version: "test".into(), allow_reboot: false },
            Box::new(Shared(sys)),
            Box::new(FakePlatform(world.clone(), hw)),
        )
        .unwrap();
        let mut ui = Self { core, world, next_id: 1, seq: 0, events: vec![] };
        ui.drain();
        let snap = ui.call("state.snapshot", json!({})).unwrap();
        ui.seq = snap["seq"].as_u64().unwrap();
        ui
    }

    /// Collect frames; check every event is valid JSON with a contiguous seq.
    fn drain(&mut self) -> Vec<Value> {
        let mut replies = vec![];
        for o in self.core.take_out() {
            match o {
                Out::Reply(_, f) => replies.push(serde_json::from_str::<Value>(&f).unwrap()),
                Out::Broadcast(f) => {
                    let v: Value = serde_json::from_str(&f).unwrap();
                    let seq = v["seq"].as_u64().unwrap();
                    if self.seq != 0 {
                        assert_eq!(seq, self.seq + 1, "seq gap: {v}");
                    }
                    self.seq = seq;
                    self.events.push(v);
                }
            }
        }
        replies
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next_id;
        self.next_id += 1;
        let f = json!({ "kind": "req", "id": id, "method": method, "params": params }).to_string();
        self.core.request(1, &f);
        let replies = self.drain();
        self.core.run_queued();
        self.drain();
        let r = replies.into_iter().find(|r| r["id"] == id).expect("reply");
        if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].clone()) }
    }

    fn snap(&mut self) -> Value {
        self.call("state.snapshot", json!({})).unwrap()
    }

    fn advance(&mut self, ms: i64) {
        let end = self.world.clock.get() + ms;
        while self.world.clock.get() < end {
            self.world.clock.set(self.world.clock.get() + 500);
            self.core.tick();
            self.drain();
        }
    }

    /// Launch, play (frames come from the capture), quit.
    fn play(&mut self) {
        self.world.game_running.set(true);
        self.advance(2_500);
        assert_eq!(self.core.live_game(), Some("steam:367520"), "game detected");
        let s = self.snap();
        assert_eq!(s["activeSession"]["gameId"], "steam:367520");
        assert!(s["activeSession"]["fps"].as_f64().unwrap() > 30.0, "HUD has live FPS");
        assert_eq!(s["games"][0]["status"], "running");
        self.world.game_running.set(false);
        self.advance(2_500);
        assert_eq!(self.core.live_game(), None, "exit detected");
    }

    fn finished(&self, op: &str) -> Value {
        self.events
            .iter()
            .rev()
            .find(|e| e["type"] == "op.finished" && e["data"]["opId"] == op)
            .cloned()
            .unwrap_or_else(|| panic!("op {op} never finished"))
    }

    fn op(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).unwrap();
        let id = r["opId"].as_str().unwrap().to_string();
        let f = self.finished(&id);
        assert!(self.snap()["operations"].as_array().unwrap().is_empty(), "op gone from snapshot");
        f["data"].clone()
    }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("overdrive-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn world(sys: &Rc<FakeSystem>) -> Rc<World> {
    Rc::new(World { clock: Cell::new(1_760_000_000_000), game_running: Cell::new(false), sys: sys.clone(), rng: RefCell::new(Rng::new(11)), captures: Cell::new(0) })
}

/// Original state minus the volatile process flags the engine restores anyway.
fn same_as_original(s: &FakeState) -> bool {
    *s == original()
}

#[test]
fn full_flow_through_the_protocol() {
    let dir = tmp("full");
    let sys = Rc::new(FakeSystem::new(original()));
    let w = world(&sys);
    let mut ui = Ui::new(&dir, w.clone(), sys.clone());

    let hello = ui.call("hello", json!({ "uiVersion": "t", "locale": "es" })).unwrap();
    assert_eq!(hello["protocol"], 1);
    let s = ui.snap();
    let g = &s["games"][0];
    assert_eq!((g["id"].as_str(), g["status"].as_str(), g["antiCheat"].as_str()), (Some("steam:367520"), Some("needs_baseline"), Some("none")));
    assert_eq!(s["hardware"]["tier"].as_u64().map(|t| (1..=3).contains(&t)), Some(true));

    // Bad input never crashes and always gets an answer.
    assert_eq!(ui.call("plan.preview", json!({ "gameId": "steam:999" })).unwrap_err()["code"], "UNKNOWN_GAME");
    assert_eq!(ui.call("system.exec", json!({ "cmd": "calc" })).unwrap_err()["code"], "UNKNOWN_METHOD");
    assert_eq!(ui.call("bios.rebootToFirmware", json!({})).unwrap_err()["code"], "CONFIRM_REQUIRED");
    ui.core.request(1, "{not json");
    assert_eq!(ui.drain()[0]["ok"], false);

    // "Measure first": opt in + plain-language notice. A game the user never opted in is
    // observed only: nothing is applied.
    let f = ui.op("session.benchmark", json!({ "gameId": "steam:367520" }));
    assert_eq!(f["outcome"], "done");
    let s = ui.snap();
    assert!(s["prompts"].as_array().unwrap().iter().any(|p| p["messageKey"] == "notice.play_to_measure"));

    for _ in 0..3 {
        ui.play();
        assert!(same_as_original(&sys.snapshot()), "baseline sessions change nothing");
    }
    let s = ui.snap();
    assert_eq!(s["games"][0]["status"], "ready", "baseline done");
    assert!(s["prompts"].as_array().unwrap().iter().any(|p| p["messageKey"] == "notice.baseline_saved"));

    // Plan (Layer 2).
    let plan = ui.call("plan.preview", json!({ "gameId": "steam:367520" })).unwrap();
    let items = plan["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["tweakId"] == "hybrid_gpu" && i["enabled"] == true), "{plan}");
    assert!(plan["hiddenInEasy"].as_u64().unwrap() > 0, "easy mode hides risky tweaks");

    // Optimize → A/B until something is kept and offered.
    let mut offered = None;
    let mut reports = 0;
    for _ in 0..30 {
        let f = ui.op("session.optimize", json!({ "gameId": "steam:367520" }));
        assert_eq!(f["outcome"], "done", "{f}");
        assert_eq!(ui.snap()["games"][0]["status"], "armed");
        ui.play();
        let st = sys.snapshot();
        assert!(!st.procs[&100].eco_qos && !st.procs[&200].suspended && st.power == BALANCED, "session reverted");
        if ui.events.iter().any(|e| e["type"] == "session.report") {
            reports += 1;
        }
        let s = ui.snap();
        if let Some(p) = s["prompts"].as_array().unwrap().iter().find(|p| p["kind"] == "keep_persistent") {
            offered = Some(p.clone());
            break;
        }
    }
    assert!(reports > 0, "measured sessions produce reports");
    let offer = offered.expect("a proven persistent win is offered");
    let kept = offer["tweakId"].as_str().unwrap().to_string();
    assert!(["hybrid_gpu", "gamedvr_off"].contains(&kept.as_str()), "only real wins are offered: {offer}");
    assert!(offer["deltaPct"].as_f64().unwrap() > 3.0);

    // The report is real data.
    let s = ui.snap();
    let tx = s["reports"][0]["txId"].as_i64().unwrap();
    let rep = ui.call("report.session", json!({ "txId": tx })).unwrap();
    assert!(rep["baselineRuns"].as_u64().unwrap() >= 3);
    assert!(rep["optimizedSeries"].as_array().unwrap().len() > 100);
    assert!(rep["baselineSeries"].as_array().unwrap().len() > 100);
    assert!(rep["optimized"]["low1"].as_f64().unwrap() > rep["baseline"]["low1"].as_f64().unwrap());

    // "Yes, keep it" → persistent, survives outside sessions.
    ui.call("prompt.answer", json!({ "promptId": offer["promptId"], "choice": "yes" })).unwrap();
    let s = ui.snap();
    assert!(s["persistentChanges"].as_array().unwrap().iter().any(|c| c["tweakId"] == kept.as_str()));
    assert!(!same_as_original(&sys.snapshot()));

    // Contract fixture for the UI test suite.
    let fixture = json!({ "snapshot": s, "plan": plan, "report": rep });
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/web/test/fixtures/svc-snapshot.json");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, serde_json::to_string_pretty(&fixture).unwrap()).unwrap();

    // Revert that one.
    let f = ui.op("persistent.revert", json!({ "tweakId": kept }));
    assert_eq!(f["outcome"], "done", "{f}");
    assert!(same_as_original(&sys.snapshot()), "revert one = back to original");

    // Global reset leaves the machine exactly as found.
    ui.op("session.optimize", json!({ "gameId": "steam:367520" }));
    let f = ui.op("tx.revertAll", json!({}));
    assert_eq!(f["outcome"], "done", "{f}");
    assert!(same_as_original(&sys.snapshot()));
    assert_ne!(ui.snap()["games"][0]["status"], "armed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn crash_while_playing_is_recovered_on_restart() {
    let dir = tmp("crash");
    let sys = Rc::new(FakeSystem::new(original()));
    let w = world(&sys);
    {
        let mut ui = Ui::new(&dir, w.clone(), sys.clone());
        ui.op("session.benchmark", json!({ "gameId": "steam:367520" }));
        for _ in 0..3 {
            ui.play();
        }
        ui.op("session.optimize", json!({ "gameId": "steam:367520" }));
        w.game_running.set(true);
        ui.advance(2_500);
        assert!(!same_as_original(&sys.snapshot()), "session tweaks applied");
        // The service dies here: no shutdown(), no exit handling.
    }
    w.game_running.set(false);
    let mut ui = Ui::new(&dir, w.clone(), sys.clone());
    assert!(same_as_original(&sys.snapshot()), "restart put everything back");
    let s = ui.snap();
    assert!(s["prompts"].as_array().unwrap().iter().any(|p| p["kind"] == "recovered"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toggles_and_settings_are_validated_and_persisted() {
    let dir = tmp("toggles");
    let sys = Rc::new(FakeSystem::new(original()));
    let w = world(&sys);
    let mut ui = Ui::new(&dir, w.clone(), sys.clone());
    let p = ui.call("plan.setToggle", json!({ "gameId": "steam:367520", "tweakId": "hybrid_gpu", "enabled": false })).unwrap();
    let it = p["items"].as_array().unwrap().iter().find(|i| i["tweakId"] == "hybrid_gpu").unwrap().clone();
    assert_eq!(it["enabled"], false);
    assert!(ui.call("settings.set", json!({ "mode": "root" })).is_err());
    assert!(ui.call("settings.set", json!({ "evil": 1 })).is_err());
    ui.call("settings.set", json!({ "mode": "expert", "locale": "en" })).unwrap();
    drop(ui);
    let mut ui = Ui::new(&dir, w, sys);
    let s = ui.snap();
    assert_eq!((s["settings"]["mode"].as_str(), s["settings"]["locale"].as_str()), (Some("expert"), Some("en")));
    let p = ui.call("plan.preview", json!({ "gameId": "steam:367520" })).unwrap();
    assert!(p["items"].as_array().unwrap().iter().any(|i| i["tweakId"] == "hybrid_gpu" && i["enabled"] == false));
    let _ = HashSet::<u8>::new();
    let _ = std::fs::remove_dir_all(&dir);
}
