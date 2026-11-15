//! `WinSystem`: the real `SystemOps` (registry, services, power, processes).
//!
//! Process methods always re-check the process identity (PID + creation time) before
//! acting, so a reused PID can never be touched by mistake. Handles are requested with the
//! minimum rights and closed immediately.

use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, FILETIME, HANDLE,
    STILL_ACTIVE, WIN32_ERROR,
};
use windows::Win32::System::Power::PowerSetActiveScheme;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW,
    RegQueryInfoKeyW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS,
    KEY_READ, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceConfigW,
    ENUM_SERVICE_TYPE, QUERY_SERVICE_CONFIGW, SC_MANAGER_CONNECT, SERVICE_CHANGE_CONFIG,
    SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG, SERVICE_START_TYPE,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetPriorityClass, GetProcessInformation, GetProcessTimes, OpenProcess,
    ProcessPowerThrottling, QueryFullProcessImageNameW, SetPriorityClass, SetProcessInformation,
    PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS, PROCESS_NAME_WIN32,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SUSPEND_RESUME,
};

use overdrive_core::engine::{
    PriorityClass, ProcRef, RegData, RegRoot, StartType, SysError, SysResult, SystemOps,
};

#[link(name = "ntdll")]
extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

fn root(r: &RegRoot, key: &str) -> (HKEY, String) {
    match r {
        RegRoot::LocalMachine => (HKEY_LOCAL_MACHINE, key.to_string()),
        RegRoot::CurrentUser => (HKEY_CURRENT_USER, key.to_string()),
        RegRoot::User(sid) => (HKEY_USERS, format!("{sid}\\{key}")),
    }
}

fn werr(what: &str, e: WIN32_ERROR) -> SysError {
    match e {
        ERROR_ACCESS_DENIED => SysError::AccessDenied(what.into()),
        ERROR_FILE_NOT_FOUND => SysError::NotFound(what.into()),
        _ => SysError::Other(format!("{what}: Win32 error {}", e.0)),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Proc(HANDLE);
impl Drop for Proc {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

/// Opens a process only if it is still *the same* process (PID + creation time) and alive.
/// `Ok(None)` = it exited or the PID was reused.
fn open(p: &ProcRef, rights: PROCESS_ACCESS_RIGHTS) -> SysResult<Option<Proc>> {
    let h = match unsafe { OpenProcess(rights | PROCESS_QUERY_LIMITED_INFORMATION, false, p.pid) } {
        Ok(h) => Proc(h),
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            return Err(SysError::AccessDenied(format!("pid {}", p.pid)))
        }
        Err(_) => return Ok(None), // invalid parameter: no such process
    };
    let (mut c, mut x, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(h.0, &mut c, &mut x, &mut k, &mut u) }
        .map_err(|e| SysError::Other(e.to_string()))?;
    if ft(c) != p.start_time {
        return Ok(None);
    }
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(h.0, &mut code) }.map_err(|e| SysError::Other(e.to_string()))?;
    Ok((code == STILL_ACTIVE.0 as u32).then_some(h))
}

fn must_open(p: &ProcRef, rights: PROCESS_ACCESS_RIGHTS) -> SysResult<Proc> {
    open(p, rights)?.ok_or_else(|| SysError::NotFound(format!("pid {} ({})", p.pid, p.image)))
}

fn class_from(v: u32) -> SysResult<PriorityClass> {
    Ok(match v {
        0x40 => PriorityClass::Idle,
        0x4000 => PriorityClass::BelowNormal,
        0x20 => PriorityClass::Normal,
        0x8000 => PriorityClass::AboveNormal,
        0x80 => PriorityClass::High,
        0x100 => PriorityClass::Realtime,
        0 => return Err(SysError::Other("GetPriorityClass failed".into())),
        x => return Err(SysError::Other(format!("unknown priority class 0x{x:x}"))),
    })
}

fn class_to(c: PriorityClass) -> u32 {
    match c {
        PriorityClass::Idle => 0x40,
        PriorityClass::BelowNormal => 0x4000,
        PriorityClass::Normal => 0x20,
        PriorityClass::AboveNormal => 0x8000,
        PriorityClass::High => 0x80,
        PriorityClass::Realtime => 0x100,
    }
}

fn start_from(v: u32) -> SysResult<StartType> {
    Ok(match v {
        0 => StartType::Boot,
        1 => StartType::System,
        2 => StartType::Automatic,
        3 => StartType::Manual,
        4 => StartType::Disabled,
        x => return Err(SysError::Other(format!("unknown service start type {x}"))),
    })
}

fn start_to(s: StartType) -> u32 {
    match s {
        StartType::Boot => 0,
        StartType::System => 1,
        StartType::Automatic => 2,
        StartType::Manual => 3,
        StartType::Disabled => 4,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WinSystem;

impl WinSystem {
    /// Builds a `ProcRef` (with creation time) for a running PID — what the policy layer uses.
    pub fn proc_ref(pid: u32) -> SysResult<ProcRef> {
        let h = Proc(
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
                .map_err(|e| SysError::Other(e.to_string()))?,
        );
        let (mut c, mut x, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe { GetProcessTimes(h.0, &mut c, &mut x, &mut k, &mut u) }
            .map_err(|e| SysError::Other(e.to_string()))?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
        }
        .map_err(|e| SysError::Other(e.to_string()))?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let image = path.rsplit('\\').next().unwrap_or(&path).to_string();
        Ok(ProcRef {
            pid,
            start_time: ft(c),
            image,
        })
    }

    /// Deletes a whole key tree. Only for cleaning up test sandboxes.
    pub fn delete_tree(&self, r: &RegRoot, key: &str) -> SysResult<()> {
        let (hk, path) = root(r, key);
        match unsafe { RegDeleteTreeW(hk, &HSTRING::from(path)) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => {}
            e => return Err(werr(key, e)),
        }
        match unsafe { RegDeleteKeyW(hk, &HSTRING::from(root(r, key).1)) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(werr(key, e)),
        }
    }

    fn with_service<T>(
        name: &str,
        rights: u32,
        f: impl FnOnce(windows::Win32::System::Services::SC_HANDLE) -> SysResult<T>,
    ) -> SysResult<T> {
        unsafe {
            let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT)
                .map_err(|e| SysError::Other(format!("OpenSCManager: {e}")))?;
            let svc = OpenServiceW(scm, &HSTRING::from(name), rights);
            let out = match svc {
                Ok(s) => {
                    let r = f(s);
                    let _ = CloseServiceHandle(s);
                    r
                }
                Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => {
                    Err(SysError::AccessDenied(format!("service {name}")))
                }
                Err(e) => Err(SysError::NotFound(format!("service {name}: {e}"))),
            };
            let _ = CloseServiceHandle(scm);
            out
        }
    }
}

impl SystemOps for WinSystem {
    fn reg_read(&self, r: &RegRoot, key: &str, name: &str) -> SysResult<Option<RegData>> {
        let (hk, path) = root(r, key);
        let (p, n) = (HSTRING::from(path), HSTRING::from(name));
        let mut ty = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let flags = RRF_RT_REG_DWORD | RRF_RT_REG_SZ;
        match unsafe { RegGetValueW(hk, &p, &n, flags, Some(&mut ty), None, Some(&mut size)) } {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            e => return Err(werr(&format!("{key}\\{name}"), e)),
        }
        let mut buf = vec![0u8; size as usize + 2];
        let mut size2 = buf.len() as u32;
        match unsafe {
            RegGetValueW(
                hk,
                &p,
                &n,
                flags,
                Some(&mut ty),
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut size2),
            )
        } {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            e => return Err(werr(&format!("{key}\\{name}"), e)),
        }
        Ok(Some(if ty == REG_DWORD {
            RegData::Dword(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
        } else if ty == REG_SZ {
            let w: Vec<u16> = buf[..size2 as usize]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
            RegData::String(String::from_utf16_lossy(&w[..end]))
        } else {
            return Err(SysError::Other(format!(
                "{key}\\{name}: unsupported registry type {}",
                ty.0
            )));
        }))
    }

    fn reg_write(&self, r: &RegRoot, key: &str, name: &str, data: &RegData) -> SysResult<()> {
        let (hk, path) = root(r, key);
        let (p, n) = (HSTRING::from(path), HSTRING::from(name));
        let e = unsafe {
            match data {
                RegData::Dword(v) => RegSetKeyValueW(
                    hk,
                    &p,
                    &n,
                    REG_DWORD.0,
                    Some(v as *const u32 as *const _),
                    4,
                ),
                RegData::String(s) => {
                    let w = wide(s);
                    RegSetKeyValueW(
                        hk,
                        &p,
                        &n,
                        REG_SZ.0,
                        Some(w.as_ptr() as *const _),
                        (w.len() * 2) as u32,
                    )
                }
            }
        };
        if e == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(werr(&format!("{key}\\{name}"), e))
        }
    }

    fn reg_delete(&self, r: &RegRoot, key: &str, name: &str) -> SysResult<()> {
        let (hk, path) = root(r, key);
        match unsafe { RegDeleteKeyValueW(hk, &HSTRING::from(path), &HSTRING::from(name)) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(werr(&format!("{key}\\{name}"), e)),
        }
    }

    fn reg_key_exists(&self, r: &RegRoot, key: &str) -> SysResult<bool> {
        let (hk, path) = root(r, key);
        let mut out = HKEY::default();
        match unsafe { RegOpenKeyExW(hk, &HSTRING::from(path), None, KEY_READ, &mut out) } {
            ERROR_SUCCESS => {
                unsafe {
                    let _ = RegCloseKey(out);
                }
                Ok(true)
            }
            ERROR_FILE_NOT_FOUND => Ok(false),
            e => Err(werr(key, e)),
        }
    }

    fn reg_delete_key_if_empty(&self, r: &RegRoot, key: &str) -> SysResult<()> {
        let (hk, path) = root(r, key);
        let p = HSTRING::from(path);
        let mut h = HKEY::default();
        match unsafe { RegOpenKeyExW(hk, &p, None, KEY_READ, &mut h) } {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(()),
            e => return Err(werr(key, e)),
        }
        let (mut subkeys, mut values) = (0u32, 0u32);
        let q = unsafe {
            RegQueryInfoKeyW(
                h,
                None,
                None,
                None,
                Some(&mut subkeys),
                None,
                None,
                Some(&mut values),
                None,
                None,
                None,
                None,
            )
        };
        unsafe {
            let _ = RegCloseKey(h);
        }
        if q != ERROR_SUCCESS {
            return Err(werr(key, q));
        }
        if subkeys > 0 || values > 0 {
            return Ok(()); // never delete anything that holds data
        }
        match unsafe { RegDeleteKeyW(hk, &p) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(werr(key, e)),
        }
    }

    fn service_start_type(&self, name: &str) -> SysResult<StartType> {
        Self::with_service(name, SERVICE_QUERY_CONFIG, |s| unsafe {
            let mut need = 0u32;
            let _ = QueryServiceConfigW(s, None, 0, &mut need);
            let mut buf =
                vec![
                    0u8;
                    need.max(std::mem::size_of::<QUERY_SERVICE_CONFIGW>() as u32) as usize + 8
                ];
            QueryServiceConfigW(
                s,
                Some(buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW),
                buf.len() as u32,
                &mut need,
            )
            .map_err(|e| SysError::Other(format!("QueryServiceConfig: {e}")))?;
            let cfg = &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW);
            start_from(cfg.dwStartType.0)
        })
    }

    fn set_service_start_type(&self, name: &str, start: StartType) -> SysResult<()> {
        Self::with_service(name, SERVICE_CHANGE_CONFIG, |s| unsafe {
            ChangeServiceConfigW(
                s,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                SERVICE_START_TYPE(start_to(start)),
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .map_err(|e| {
                if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
                    SysError::AccessDenied(format!("service {name}"))
                } else {
                    SysError::Other(e.to_string())
                }
            })
        })
    }

    fn active_power_scheme(&self) -> SysResult<String> {
        crate::sys::active_power_scheme()
            .ok_or_else(|| SysError::Other("PowerGetActiveScheme failed".into()))
    }

    fn set_power_scheme(&self, guid: &str) -> SysResult<()> {
        let g = GUID::try_from(guid.trim_matches(['{', '}']))
            .map_err(|e| SysError::Other(format!("bad GUID {guid}: {e}")))?;
        match unsafe { PowerSetActiveScheme(None, Some(&g)) } {
            ERROR_SUCCESS => Ok(()),
            e => Err(werr("PowerSetActiveScheme", e)),
        }
    }

    fn power_schemes(&self) -> SysResult<Vec<String>> {
        use windows::Win32::System::Power::{PowerEnumerate, ACCESS_SCHEME};
        let mut out = Vec::new();
        for i in 0..64u32 {
            let mut g = GUID::zeroed();
            let mut size = std::mem::size_of::<GUID>() as u32;
            let r = unsafe {
                PowerEnumerate(
                    None,
                    None,
                    None,
                    ACCESS_SCHEME,
                    i,
                    Some(&mut g as *mut GUID as *mut u8),
                    &mut size,
                )
            };
            if r != ERROR_SUCCESS {
                break; // ERROR_NO_MORE_ITEMS
            }
            out.push(format!("{g:?}").to_ascii_lowercase());
        }
        Ok(out)
    }

    fn task_enabled(&self, path: &str) -> SysResult<bool> {
        tasks::with_task(path, |t| {
            unsafe { t.Enabled() }
                .map(|b| b.as_bool())
                .map_err(|e| tasks::err(path, e))
        })
    }

    fn set_task_enabled(&self, path: &str, enabled: bool) -> SysResult<()> {
        tasks::with_task(path, |t| {
            unsafe { t.SetEnabled(enabled.into()) }.map_err(|e| tasks::err(path, e))
        })
    }

    fn proc_alive(&self, p: &ProcRef) -> SysResult<bool> {
        match open(p, PROCESS_ACCESS_RIGHTS(0)) {
            Ok(h) => Ok(h.is_some()),
            Err(SysError::AccessDenied(_)) => Err(SysError::AccessDenied(format!("pid {}", p.pid))),
            Err(e) => Err(e),
        }
    }

    fn proc_priority(&self, p: &ProcRef) -> SysResult<PriorityClass> {
        let h = must_open(p, PROCESS_ACCESS_RIGHTS(0))?;
        class_from(unsafe { GetPriorityClass(h.0) })
    }

    fn set_proc_priority(&self, p: &ProcRef, c: PriorityClass) -> SysResult<()> {
        let h = must_open(p, PROCESS_SET_INFORMATION)?;
        unsafe { SetPriorityClass(h.0, PROCESS_CREATION_FLAGS(class_to(c))) }
            .map_err(|e| SysError::Other(e.to_string()))
    }

    fn proc_eco_qos(&self, p: &ProcRef) -> SysResult<Option<bool>> {
        let h = must_open(p, PROCESS_ACCESS_RIGHTS(0))?;
        let mut st = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ..Default::default()
        };
        let ok = unsafe {
            GetProcessInformation(
                h.0,
                ProcessPowerThrottling,
                &mut st as *mut _ as *mut _,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        };
        if ok.is_err() {
            return Ok(None); // Windows 10: settable but not queryable
        }
        Ok(Some(
            st.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
                && st.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0,
        ))
    }

    fn set_proc_eco_qos(&self, p: &ProcRef, on: bool) -> SysResult<()> {
        let h = must_open(p, PROCESS_SET_INFORMATION)?;
        // on  → explicitly throttle (EcoQoS)
        // off → back to "let Windows decide" (ControlMask 0), the default state
        let st = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: if on {
                PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            } else {
                0
            },
            StateMask: if on {
                PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            } else {
                0
            },
        };
        unsafe {
            SetProcessInformation(
                h.0,
                ProcessPowerThrottling,
                &st as *const _ as *const _,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        }
        .map_err(|e| SysError::Other(e.to_string()))
    }

    fn suspend(&self, p: &ProcRef) -> SysResult<()> {
        let h = must_open(p, PROCESS_SUSPEND_RESUME)?;
        match unsafe { NtSuspendProcess(h.0) } {
            0 => Ok(()),
            s => Err(SysError::Other(format!("NtSuspendProcess 0x{s:08x}"))),
        }
    }

    fn resume(&self, p: &ProcRef) -> SysResult<()> {
        let h = must_open(p, PROCESS_SUSPEND_RESUME)?;
        match unsafe { NtResumeProcess(h.0) } {
            0 => Ok(()),
            s => Err(SysError::Other(format!("NtResumeProcess 0x{s:08x}"))),
        }
    }
}

/// Task Scheduler over COM (ITaskService). COM is initialised per calling thread
/// (multithreaded apartment); an already-initialised thread is fine.
mod tasks {
    use overdrive_core::engine::{SysError, SysResult};
    use windows::core::BSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::TaskScheduler::{IRegisteredTask, ITaskService, TaskScheduler};
    use windows::Win32::System::Variant::VARIANT;

    pub fn err(path: &str, e: windows::core::Error) -> SysError {
        match e.code().0 as u32 {
            0x8007_0002 | 0x8007_0003 => SysError::NotFound(format!("task {path}")),
            0x8007_0005 => SysError::AccessDenied(format!("task {path}")),
            _ => SysError::Other(format!("task {path}: {e}")),
        }
    }

    pub fn with_task<T>(
        path: &str,
        f: impl FnOnce(&IRegisteredTask) -> SysResult<T>,
    ) -> SysResult<T> {
        let (folder, name) = path
            .rsplit_once('\\')
            .ok_or_else(|| SysError::Other(format!("bad task path {path}")))?;
        let folder = if folder.is_empty() { r"\" } else { folder };
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| err(path, e))?;
            let v = VARIANT::default();
            svc.Connect(&v, &v, &v, &v).map_err(|e| err(path, e))?;
            let folder = svc
                .GetFolder(&BSTR::from(folder))
                .map_err(|e| err(path, e))?;
            let task = folder
                .GetTask(&BSTR::from(name))
                .map_err(|e| err(path, e))?;
            f(&task)
        }
    }
}
