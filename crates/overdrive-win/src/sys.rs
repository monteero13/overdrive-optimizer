//! Direct Win32 calls. The only `unsafe` code in Overdrive lives here.
//! All read-only except `reboot_to_firmware`. No kernel drivers, no handles to other processes.

use std::time::Duration;

use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::DEVPROPTYPE;
use windows::Win32::Foundation::{LocalFree, DEVPROPKEY, ERROR_SUCCESS, HLOCAL};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use windows::Win32::System::EventLog::{
    EvtClose, EvtNext, EvtQuery, EvtQueryChannelPath, EvtQueryReverseDirection, EVT_HANDLE,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, PowerGetActiveScheme, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    HKEY_USERS, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};

use overdrive_core::source::{RawPcieLink, RawPerfSample, RawStabilityEvents};

// ---------------------------------------------------------------- registry

/// Whose per-user settings to read. The service runs as LocalSystem, where HKCU is
/// SYSTEM's own hive — it must read the interactive user's hive under HKEY_USERS\<SID>.
#[derive(Debug, Clone)]
pub enum UserHive {
    CurrentUser,
    Sid(String),
}

#[derive(Debug, Clone, Copy)]
pub enum Root {
    Machine,
    User,
}

fn resolve(root: Root, hive: &UserHive, sub: &str) -> (HKEY, String) {
    match (root, hive) {
        (Root::Machine, _) => (HKEY_LOCAL_MACHINE, sub.to_string()),
        (Root::User, UserHive::CurrentUser) => (HKEY_CURRENT_USER, sub.to_string()),
        (Root::User, UserHive::Sid(sid)) => (HKEY_USERS, format!("{sid}\\{sub}")),
    }
}

pub fn reg_dword(root: Root, hive: &UserHive, sub: &str, name: &str) -> Option<u32> {
    let (hk, path) = resolve(root, hive, sub);
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            hk,
            &HSTRING::from(path),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(data)
}

pub fn reg_string(root: Root, hive: &UserHive, sub: &str, name: &str) -> Option<String> {
    let (hk, path) = resolve(root, hive, sub);
    let (p, n) = (HSTRING::from(path), HSTRING::from(name));
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(hk, &p, &n, RRF_RT_REG_SZ, None, None, Some(&mut size)) != ERROR_SUCCESS
            || size == 0
        {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        if RegGetValueW(
            hk,
            &p,
            &n,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

pub fn reg_key_exists(root: Root, hive: &UserHive, sub: &str) -> bool {
    let (hk, path) = resolve(root, hive, sub);
    let mut out = HKEY::default();
    unsafe {
        let ok = RegOpenKeyExW(hk, &HSTRING::from(path), None, KEY_READ, &mut out) == ERROR_SUCCESS;
        if ok {
            let _ = RegCloseKey(out);
        }
        ok
    }
}

// ---------------------------------------------------------------- CPU topology

/// EfficiencyClass per physical core. On non-hybrid CPUs every core reports 0.
pub fn core_efficiency_classes() -> Option<Vec<u8>> {
    unsafe {
        let mut len = 0u32;
        let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        GetLogicalProcessorInformationEx(
            RelationProcessorCore,
            Some(buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX),
            &mut len,
        )
        .ok()?;
        let mut out = Vec::new();
        let mut off = 0usize;
        while off < len as usize {
            let info = &*(buf.as_ptr().add(off) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX);
            if info.Relationship == RelationProcessorCore {
                out.push(info.Anonymous.Processor.EfficiencyClass);
            }
            if info.Size == 0 {
                break;
            }
            off += info.Size as usize;
        }
        Some(out)
    }
}

// ---------------------------------------------------------------- DXGI

pub struct DxgiAdapter {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_bytes: u64,
    pub shared_bytes: u64,
}

pub fn dxgi_adapters() -> windows::core::Result<Vec<DxgiAdapter>> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut out = Vec::new();
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let d = adapter.GetDesc1()?;
            if d.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let end = d
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(d.Description.len());
            out.push(DxgiAdapter {
                name: String::from_utf16_lossy(&d.Description[..end]),
                vendor_id: d.VendorId,
                device_id: d.DeviceId,
                dedicated_bytes: d.DedicatedVideoMemory as u64,
                shared_bytes: d.SharedSystemMemory as u64,
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- PCIe link (cfgmgr32)

// DEVPKEY_PciDevice_* (pciprop.h): {3AB22E31-8264-4B4E-9AF5-A8D2D8E33E62}
const PCI_FMTID: GUID = GUID::from_u128(0x3ab22e31_8264_4b4e_9af5_a8d2d8e33e62);
const PID_CURRENT_LINK_SPEED: u32 = 9;
const PID_CURRENT_LINK_WIDTH: u32 = 10;
const PID_MAX_LINK_SPEED: u32 = 11;
const PID_MAX_LINK_WIDTH: u32 = 12;

fn devnode_u32(inst: u32, pid: u32) -> Option<u32> {
    let key = DEVPROPKEY {
        fmtid: PCI_FMTID,
        pid,
    };
    let mut ty = DEVPROPTYPE::default();
    let mut val = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        CM_Get_DevNode_PropertyW(
            inst,
            &key,
            &mut ty,
            Some(&mut val as *mut u32 as *mut u8),
            &mut size,
            0,
        )
    };
    (r == CR_SUCCESS && size == 4).then_some(val)
}

pub fn pcie_link(pnp_device_id: &str) -> Option<RawPcieLink> {
    let mut inst = 0u32;
    let id = HSTRING::from(pnp_device_id);
    let r = unsafe { CM_Locate_DevNodeW(&mut inst, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) };
    if r != CR_SUCCESS {
        return None;
    }
    let link = RawPcieLink {
        current_width: devnode_u32(inst, PID_CURRENT_LINK_WIDTH),
        max_width: devnode_u32(inst, PID_MAX_LINK_WIDTH),
        current_gen: devnode_u32(inst, PID_CURRENT_LINK_SPEED),
        max_gen: devnode_u32(inst, PID_MAX_LINK_SPEED),
    };
    (link.current_width.is_some() || link.max_width.is_some()).then_some(link)
}

// ---------------------------------------------------------------- PDH (CPU performance limit)

/// Samples the CPU performance-limit counters over `window`. Blocking: run on a thread.
pub fn perf_sample(window: Duration) -> Result<RawPerfSample, String> {
    unsafe {
        let mut q = PDH_HQUERY::default();
        let s = PdhOpenQueryW(PCWSTR::null(), 0, &mut q);
        if s != 0 {
            return Err(format!("PdhOpenQueryW 0x{s:08x}"));
        }
        let add = |path: &str| -> Option<PDH_HCOUNTER> {
            let mut c = PDH_HCOUNTER::default();
            (PdhAddEnglishCounterW(q, &HSTRING::from(path), 0, &mut c) == 0).then_some(c)
        };
        let limit = add("\\Processor Information(_Total)\\% Performance Limit");
        let perf = add("\\Processor Information(_Total)\\% Processor Performance");
        PdhCollectQueryData(q);
        std::thread::sleep(window);
        PdhCollectQueryData(q);
        let read = |c: Option<PDH_HCOUNTER>| -> Option<f64> {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            (PdhGetFormattedCounterValue(c?, PDH_FMT_DOUBLE, None, &mut v) == 0)
                .then_some(v.Anonymous.doubleValue)
        };
        let out = RawPerfSample {
            perf_limit_pct: read(limit),
            processor_performance_pct: read(perf),
        };
        PdhCloseQuery(q);
        Ok(out)
    }
}

// ---------------------------------------------------------------- power

pub fn power_status() -> Option<(Option<bool>, bool)> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    let ac = match s.ACLineStatus {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    // BatteryFlag 128 = no system battery; 255 = unknown.
    let has_battery = s.BatteryFlag != 128 && s.BatteryFlag != 255;
    Some((ac, has_battery))
}

pub fn active_power_scheme() -> Option<String> {
    unsafe {
        let mut p: *mut GUID = std::ptr::null_mut();
        if PowerGetActiveScheme(None, &mut p) != ERROR_SUCCESS || p.is_null() {
            return None;
        }
        let g = *p;
        let _ = LocalFree(Some(HLOCAL(p as *mut _)));
        Some(format!("{g:?}").to_ascii_lowercase())
    }
}

// ---------------------------------------------------------------- event log (BIOS Coach stability)

fn count_events(xpath: &str) -> Result<u32, String> {
    unsafe {
        let q = EvtQuery(
            None,
            &HSTRING::from("System"),
            &HSTRING::from(xpath),
            EvtQueryChannelPath.0 | EvtQueryReverseDirection.0,
        )
        .map_err(|e| format!("EvtQuery: {e}"))?;
        let mut total = 0u32;
        let mut batch = [0isize; 64];
        loop {
            let mut n = 0u32;
            if EvtNext(q, &mut batch, 0, 0, &mut n).is_err() || n == 0 {
                break;
            }
            for h in &batch[..n as usize] {
                let _ = EvtClose(EVT_HANDLE(*h));
            }
            total += n;
        }
        let _ = EvtClose(q);
        Ok(total)
    }
}

/// WHEA hardware errors, unexpected shutdowns (Kernel-Power 41) and bugchecks (1001)
/// in the last `window_ms`.
pub fn stability_events(window_ms: u64) -> Result<RawStabilityEvents, String> {
    let t = format!("TimeCreated[timediff(@SystemTime) <= {window_ms}]");
    Ok(RawStabilityEvents {
        whea_errors: count_events(&format!("*[System[Provider[@Name='Microsoft-Windows-WHEA-Logger'] and (Level=1 or Level=2 or Level=3) and {t}]]"))?,
        unexpected_shutdowns: count_events(&format!("*[System[Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=41 and {t}]]"))?,
        bugchecks: count_events(&format!("*[System[Provider[@Name='Microsoft-Windows-WER-SystemErrorReporting'] and EventID=1001 and {t}]]"))?,
    })
}

// ---------------------------------------------------------------- firmware reboot (BIOS Coach)

/// Restart straight into UEFI setup. Requires admin (the service runs as LocalSystem).
/// Never called by the profiler or the probe.
pub fn reboot_to_firmware() -> std::io::Result<()> {
    let status = std::process::Command::new("shutdown.exe")
        .args(["/r", "/fw", "/t", "0"])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "shutdown /r /fw exited with {status}"
        )))
    }
}
