//! Process enumeration for the background-isolation policy.
//!
//! Builds `overdrive_core::safety::ProcInfo` for every process the current user could see:
//! session, parent, foreground, visible window and — crucially — whether it is producing
//! or capturing audio right now (WASAPI sessions on every active endpoint). A process whose
//! creation time can't be read is left out entirely: without an identity it is never touched.
//! Enumeration reads the kernel's process list: no process handle is ever opened here.

use std::collections::HashSet;

use overdrive_core::engine::ProcRef;
use overdrive_core::safety::ProcInfo;
use windows::core::{Interface, BOOL};
use std::ffi::c_void;

use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::Win32::Media::Audio::{
    eAll, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsWindowVisible,
    GW_OWNER,
};

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

/// One entry of the kernel's process list. Read with `NtQuerySystemInformation`, so
/// **no handle is opened to any process**, not even to a game protected by a kernel
/// anti-cheat (goals.md §5.4: we never touch the game process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProc {
    pub pid: u32,
    pub parent_pid: u32,
    pub session_id: u32,
    /// FILETIME of the process creation (identity against PID reuse).
    pub start_time: u64,
    pub image: String,
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

/// Prefix of SYSTEM_PROCESS_INFORMATION (x64 layout, stable since Windows XP).
#[repr(C)]
struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
}

#[repr(C)]
struct SystemProcessIdInformation {
    process_id: usize,
    image_name: UnicodeString,
}

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const SYSTEM_PROCESS_ID_INFORMATION: u32 = 88;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

/// Every process on the machine, without opening any of them.
pub fn snapshot() -> Result<Vec<RawProc>, String> {
    let mut len: u32 = 512 * 1024;
    loop {
        // u64 storage keeps the buffer 8-byte aligned for the struct reads below.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let mut needed = 0u32;
        let st = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION,
                buf.as_mut_ptr().cast(),
                len,
                &mut needed,
            )
        };
        if st == STATUS_INFO_LENGTH_MISMATCH {
            len = needed.max(len) + 64 * 1024;
            continue;
        }
        if st < 0 {
            return Err(format!("NtQuerySystemInformation 0x{:08X}", st as u32));
        }
        let base = buf.as_ptr() as *const u8;
        let mut out = Vec::new();
        let mut off = 0usize;
        loop {
            // SAFETY: the kernel filled `buf` with a chain of entries linked by
            // next_entry_offset; every offset stays inside the returned length.
            let e = unsafe { &*(base.add(off) as *const SystemProcessInformation) };
            let pid = e.unique_process_id as u32;
            if pid != 0 {
                let image = if e.image_name.buffer.is_null() {
                    String::new()
                } else {
                    let n = e.image_name.length as usize / 2;
                    String::from_utf16_lossy(unsafe {
                        std::slice::from_raw_parts(e.image_name.buffer, n)
                    })
                };
                out.push(RawProc {
                    pid,
                    parent_pid: e.inherited_from_unique_process_id as u32,
                    session_id: e.session_id,
                    start_time: e.create_time as u64,
                    image,
                });
            }
            if e.next_entry_offset == 0 {
                break;
            }
            off += e.next_entry_offset as usize;
        }
        return Ok(out);
    }
}

/// Full Win32 path of a process image (`C:\Games\...\game.exe`), again without a handle:
/// `SystemProcessIdInformation` returns the NT path, mapped back to a drive letter.
pub fn image_path(pid: u32) -> Option<String> {
    let mut wbuf = vec![0u16; 1024];
    let mut info = SystemProcessIdInformation {
        process_id: pid as usize,
        image_name: UnicodeString {
            length: 0,
            maximum_length: (wbuf.len() * 2) as u16,
            buffer: wbuf.as_mut_ptr(),
        },
    };
    let st = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESS_ID_INFORMATION,
            (&mut info as *mut SystemProcessIdInformation).cast(),
            std::mem::size_of::<SystemProcessIdInformation>() as u32,
            std::ptr::null_mut(),
        )
    };
    if st < 0 {
        return None;
    }
    let nt = String::from_utf16_lossy(&wbuf[..info.image_name.length as usize / 2]);
    nt_to_dos(&nt)
}

/// `\Device\HarddiskVolume3\x` → `C:\x` using the current drive-letter mapping.
fn nt_to_dos(nt: &str) -> Option<String> {
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", letter as char);
        let mut target = [0u16; 512];
        let n = unsafe { QueryDosDeviceW(&HSTRING::from(drive.as_str()), Some(&mut target)) };
        if n == 0 {
            continue;
        }
        let end = target.iter().position(|&c| c == 0).unwrap_or(n as usize);
        let dev = String::from_utf16_lossy(&target[..end]);
        if let Some(rest) = nt.strip_prefix(dev.as_str()) {
            if rest.starts_with('\\') {
                return Some(format!("{drive}{rest}"));
            }
        }
    }
    None
}

pub fn list() -> Result<Vec<ProcInfo>, String> {
    let audio = audio_pids();
    let windows = windowed_pids();
    let fg = foreground_pid();
    Ok(snapshot()?
        .into_iter()
        .filter(|p| p.start_time != 0)
        .map(|p| ProcInfo {
            is_foreground: fg == Some(p.pid),
            produces_audio: audio.contains(&p.pid),
            has_visible_window: windows.contains(&p.pid),
            session_id: p.session_id,
            parent_pid: Some(p.parent_pid),
            process: ProcRef {
                pid: p.pid,
                start_time: p.start_time,
                image: p.image,
            },
        })
        .collect())
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
