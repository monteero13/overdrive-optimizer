//! Process enumeration for the background-isolation policy.
//!
//! Builds `overdrive_core::safety::ProcInfo` for every process the current user could see:
//! session, parent, foreground, visible window and — crucially — whether it is producing
//! or capturing audio right now (WASAPI sessions on every active endpoint). A process whose
//! creation time can't be read is left out entirely: without an identity it is never touched.

use std::collections::HashSet;

use overdrive_core::engine::ProcRef;
use overdrive_core::safety::ProcInfo;
use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM};
use windows::Win32::Media::Audio::{
    eAll, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsWindowVisible,
    GW_OWNER,
};

fn creation_time(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut x, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let ok = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u).is_ok();
        let _ = CloseHandle(h);
        ok.then_some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }
}

/// PIDs with an *active* audio session (playing or recording) on any active endpoint.
pub fn audio_pids() -> HashSet<u32> {
    let mut out = HashSet::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(
            &MMDeviceEnumerator,
            None,
            CLSCTX_INPROC_SERVER,
        ) else {
            return out;
        };
        let Ok(devices) = en.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE) else {
            return out;
        };
        let n = devices.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(dev) = devices.Item(i) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = mgr.GetSessionEnumerator() else {
                continue;
            };
            for s in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(ctl) = sessions.GetSession(s) else {
                    continue;
                };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                if ctl2
                    .GetState()
                    .is_ok_and(|st| st == AudioSessionStateActive)
                {
                    if let Ok(pid) = ctl2.GetProcessId() {
                        if pid != 0 {
                            out.insert(pid);
                        }
                    }
                }
            }
        }
    }
    out
}

/// PIDs owning at least one visible top-level window.
pub fn windowed_pids() -> HashSet<u32> {
    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd, GW_OWNER).is_err() {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                (*(lp.0 as *mut HashSet<u32>)).insert(pid);
            }
        }
        BOOL(1)
    }
    let mut set: HashSet<u32> = HashSet::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut set as *mut _ as isize));
    }
    set
}

pub fn foreground_pid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

pub fn list() -> Result<Vec<ProcInfo>, String> {
    let audio = audio_pids();
    let windows = windowed_pids();
    let fg = foreground_pid();
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?;
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let pid = e.th32ProcessID;
            let end = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            let image = String::from_utf16_lossy(&e.szExeFile[..end]);
            let mut session = u32::MAX;
            let has_session = ProcessIdToSessionId(pid, &mut session).is_ok();
            if pid != 0 && has_session {
                if let Some(start_time) = creation_time(pid) {
                    out.push(ProcInfo {
                        process: ProcRef {
                            pid,
                            start_time,
                            image,
                        },
                        session_id: session,
                        parent_pid: Some(e.th32ParentProcessID),
                        is_foreground: fg == Some(pid),
                        produces_audio: audio.contains(&pid),
                        has_visible_window: windows.contains(&pid),
                    });
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    Ok(out)
}

/// PIDs of `root` and all its descendants (a game's helper processes).
pub fn descendants(procs: &[ProcInfo], root: u32) -> HashSet<u32> {
    let mut set = HashSet::from([root]);
    loop {
        let before = set.len();
        for p in procs {
            if p.parent_pid.is_some_and(|pp| set.contains(&pp)) {
                set.insert(p.process.pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}
