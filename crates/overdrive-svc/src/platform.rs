//! What the service needs from the machine, besides `SystemOps` (which changes things).
//! `win::WinPlatform` is the real one; tests drive the whole service with a scripted fake.

use overdrive_core::engine::RegRoot;
use overdrive_core::games::UserDirs;
use overdrive_core::profile::HardwareProfile;
use overdrive_core::safety::ProcInfo;
use overdrive_core::source::RawStabilityEvents;
use overdrive_core::telemetry::{FrameSample, SystemSample};

/// A game found in a launcher library.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryGame {
    /// `steam:<appid>`, `epic:<AppName>`.
    pub id: String,
    pub name: String,
    pub launcher: &'static str,
    pub install_dir: String,
    /// Executable declared by the launcher (Epic), if any.
    pub exe_hint: Option<String>,
    /// File and folder names found in the install tree (anti-cheat signatures).
    pub install_entries: Vec<String>,
}

/// Frame capture for one running game (PresentMon + 1 Hz system counters).
pub trait Capture {
    /// Frames captured since the last call (non-blocking).
    fn poll(&mut self) -> Vec<FrameSample>;
    /// Stop and return the system samples of the whole session.
    fn finish(self: Box<Self>) -> Vec<SystemSample>;
}

pub trait Platform {
    fn profile(&mut self, user: &RegRoot) -> HardwareProfile;
    fn processes(&mut self) -> Vec<ProcInfo>;
    /// (pid, start time) of every process, as cheaply as possible (no handles, no audio or
    /// window enumeration). `None` = not available: the service then always does the full
    /// scan.
    fn process_keys(&mut self) -> Option<Vec<(u32, u64)>> {
        None
    }
    /// Full image path of a process, read without opening it.
    fn image_path(&mut self, pid: u32) -> Option<String>;
    fn library(&mut self, user: &RegRoot) -> Vec<LibraryGame>;
    /// `Err` with a reason when no capture tool is available (the session is still
    /// handled and reverted, it just can't be measured).
    fn capture(&mut self, pid: u32) -> Result<Box<dyn Capture>, String>;
    /// Hive of the interactive user (`HKEY_USERS\<SID>` when running as LocalSystem).
    fn user_root(&mut self) -> RegRoot;
    /// That user's AppData / LocalAppData / Documents (never the service's own).
    fn user_dirs(&mut self, user: &RegRoot) -> UserDirs;
    fn reboot_to_firmware(&mut self) -> Result<(), String>;
    fn stability_events(&mut self, since_ms: i64) -> Option<RawStabilityEvents>;
    fn now_ms(&self) -> i64;
    /// Local hour 0–23 (quiet-hours logic in the catalog).
    fn local_hour(&self) -> u8;
}
