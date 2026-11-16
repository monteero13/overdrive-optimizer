//! The real `Platform` on Windows.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overdrive_core::build_profile;
use overdrive_core::engine::RegRoot;
use overdrive_core::games::UserDirs;
use overdrive_core::profile::HardwareProfile;
use overdrive_core::safety::ProcInfo;
use overdrive_core::source::RawStabilityEvents;
use overdrive_core::telemetry::{FrameSample, SystemSample, TelemetrySource};
use overdrive_win::sys::{self, Root};
use overdrive_win::telemetry::{PresentMonSource, SystemSampler};
use overdrive_win::{identity, processes, UserHive, WinHardwareSource};

use crate::library;
use crate::platform::{Capture, LibraryGame, Platform};

pub struct WinPlatform;

fn hive(user: &RegRoot) -> UserHive {
    match user {
        RegRoot::User(sid) => UserHive::Sid(sid.clone()),
        _ => UserHive::CurrentUser,
    }
}

/// `presentmon\PresentMon.exe` next to the service, or `OVERDRIVE_PRESENTMON` (dev builds).
pub fn presentmon_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundled = exe.parent()?.join("presentmon").join("PresentMon.exe");
    if bundled.exists() {
        return Some(bundled);
    }
    std::env::var_os("OVERDRIVE_PRESENTMON")
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

struct WinCapture {
    pm: PresentMonSource,
    sampler: Option<SystemSampler>,
}

impl Capture for WinCapture {
    fn poll(&mut self) -> Vec<FrameSample> {
        self.pm.poll()
    }
    fn finish(mut self: Box<Self>) -> Vec<SystemSample> {
        let _ = self.pm.stop();
        self.sampler.take().map(|s| s.stop().0).unwrap_or_default()
    }
}

impl Platform for WinPlatform {
    fn profile(&mut self, user: &RegRoot) -> HardwareProfile {
        build_profile(&WinHardwareSource::new(hive(user)))
    }
    fn processes(&mut self) -> Vec<ProcInfo> {
        processes::list().unwrap_or_default()
    }
    fn process_keys(&mut self) -> Option<Vec<(u32, u64)>> {
        // Only the interactive sessions: services (session 0) are never games.
        processes::snapshot()
            .ok()
            .map(|v| v.into_iter().filter(|p| p.session_id != 0).map(|p| (p.pid, p.start_time)).collect())
    }
    fn running_images(&mut self) -> Option<Vec<String>> {
        processes::snapshot()
            .ok()
            .map(|v| v.into_iter().filter(|p| p.session_id != 0).map(|p| p.image).collect())
    }
    fn image_path(&mut self, pid: u32) -> Option<String> {
        processes::image_path(pid)
    }
    fn library(&mut self, user: &RegRoot) -> Vec<LibraryGame> {
        let h = hive(user);
        let steam = sys::reg_string(Root::User, &h, r"Software\Valve\Steam", "SteamPath")
            .map(|p| PathBuf::from(p.replace('/', "\\")))
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam"));
        let pd = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
        let epic = PathBuf::from(pd).join(r"Epic\EpicGamesLauncher\Data\Manifests");
        let mut v = library::scan_steam(&steam);
        v.extend(library::scan_epic(&epic));
        let d = self.user_dirs(user);
        v.extend(library::scan_minecraft(d.appdata.as_deref(), d.local_appdata.as_deref()));
        v.sort_by_key(|a| a.name.to_lowercase());
        v
    }
    fn capture(&mut self, pid: u32) -> Result<Box<dyn Capture>, String> {
        let path = presentmon_path().ok_or("PresentMon not found")?;
        let mut pm = PresentMonSource::new(path);
        pm.start(pid).map_err(|e| e.to_string())?;
        Ok(Box::new(WinCapture {
            pm,
            sampler: Some(SystemSampler::start(Duration::from_secs(1))),
        }))
    }
    fn user_root(&mut self) -> RegRoot {
        if identity::is_local_system() {
            if let Some(sid) = identity::console_user_sid() {
                return RegRoot::User(sid);
            }
        }
        RegRoot::CurrentUser
    }
    fn sees_desktop(&self) -> bool {
        overdrive_ipc::session_of(std::process::id()).is_some_and(|s| s != 0)
    }
    fn user_dirs(&mut self, user: &RegRoot) -> UserDirs {
        // "Shell Folders" holds the expanded paths (and follows OneDrive/folder redirection).
        let h = hive(user);
        let get = |name: &str| {
            sys::reg_string(
                Root::User,
                &h,
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders",
                name,
            )
            .filter(|p| !p.is_empty())
        };
        UserDirs {
            appdata: get("AppData"),
            local_appdata: get("Local AppData"),
            documents: get("Personal"),
        }
    }
    fn reboot_to_firmware(&mut self) -> Result<(), String> {
        sys::reboot_to_firmware().map_err(|e| e.to_string())
    }
    fn stability_events(&mut self, since_ms: i64) -> Option<RawStabilityEvents> {
        let window = (self.now_ms() - since_ms).max(0) as u64;
        sys::stability_events(window).ok()
    }
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
    fn local_hour(&self) -> u8 {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        t.wHour as u8
    }
}
