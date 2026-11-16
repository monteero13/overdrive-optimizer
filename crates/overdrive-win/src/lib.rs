//! `WinHardwareSource`: the real `HardwareSource` for Windows 10/11.
//!
//! Data sources (all user-mode, read-only, no kernel drivers):
//! - WMI `ROOT\CIMV2`: Win32_Processor, Win32_PhysicalMemory(+Array), Win32_ComputerSystem,
//!   Win32_VideoController, Win32_BaseBoard, Win32_BIOS, Win32_SystemEnclosure, Win32_LogicalDisk
//! - WMI `ROOT\Microsoft\Windows\Storage`: MSFT_PhysicalDisk, MSFT_Partition
//! - DXGI (adapters, VRAM), cfgmgr32 (PCIe link), PDH (CPU performance limit),
//!   GetLogicalProcessorInformationEx (P/E cores), registry (OS + gaming settings), Event Log.

pub mod identity;
pub mod nvapi;
pub mod processes;
pub mod sys;
pub mod system_ops;
pub mod telemetry;

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overdrive_core::profiler::days_from_iso;
use overdrive_core::source::*;
use serde::Deserialize;
use wmi::WMIConnection;

pub use sys::UserHive;
use sys::{reg_dword, reg_key_exists, reg_string, Root};

/// PDH sampling window for the CPU performance-limit counter.
pub const PERF_WINDOW: Duration = Duration::from_millis(1000);

pub struct WinHardwareSource {
    cimv2: Result<WMIConnection, String>,
    storage: Result<WMIConnection, String>,
    hive: UserHive,
    perf: std::cell::RefCell<Option<mpsc::Receiver<Result<RawPerfSample, String>>>>,
}

fn err(src: &'static str) -> impl Fn(wmi::WMIError) -> SourceError {
    move |e| SourceError::new(src, e.to_string())
}

impl WinHardwareSource {
    /// Starts the 1 s PDH sample on a background thread immediately, so it overlaps
    /// with the WMI queries (keeps the whole profile under the 3 s budget).
    pub fn new(hive: UserHive) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(sys::perf_sample(PERF_WINDOW));
        });
        Self {
            cimv2: WMIConnection::new().map_err(|e| e.to_string()),
            storage: WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage")
                .map_err(|e| e.to_string()),
            hive,
            perf: std::cell::RefCell::new(Some(rx)),
        }
    }

    fn wmi(&self, src: &'static str) -> SourceResult<&WMIConnection> {
        self.cimv2
            .as_ref()
            .map_err(|e| SourceError::new(src, format!("WMI connection: {e}")))
    }

    fn query<T: for<'de> Deserialize<'de>>(
        &self,
        src: &'static str,
        q: &str,
    ) -> SourceResult<Vec<T>> {
        self.wmi(src)?.raw_query(q).map_err(err(src))
    }
}

/// WMI CIM_DATETIME "20250901000000.000000-000" → "2025-09-01".
fn cim_date(s: &str) -> Option<String> {
    let d = s.get(..8)?;
    if !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let iso = format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]);
    days_from_iso(&iso).map(|_| iso)
}

/// "PCI\VEN_10DE&DEV_1F99&SUBSYS…" → (0x10DE, 0x1F99)
fn pci_ids(pnp: &str) -> Option<(u32, u32)> {
    let up = pnp.to_ascii_uppercase();
    let hex = |tag: &str| -> Option<u32> {
        let i = up.find(tag)? + tag.len();
        u32::from_str_radix(up.get(i..i + 4)?, 16).ok()
    };
    Some((hex("VEN_")?, hex("DEV_")?))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Processor {
    name: Option<String>,
    manufacturer: Option<String>,
    number_of_cores: Option<u32>,
    number_of_logical_processors: Option<u32>,
    max_clock_speed: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PhysicalMemory {
    capacity: Option<u64>,
    speed: Option<u32>,
    configured_clock_speed: Option<u32>,
    device_locator: Option<String>,
    bank_label: Option<String>,
    part_number: Option<String>,
    manufacturer: Option<String>,
    #[serde(rename = "SMBIOSMemoryType")]
    smbios_memory_type: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MemoryArray {
    memory_devices: Option<u32>,
    #[serde(rename = "Use")]
    use_: Option<u16>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ComputerSystem {
    total_physical_memory: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VideoController {
    name: Option<String>,
    driver_version: Option<String>,
    driver_date: Option<String>,
    #[serde(rename = "PNPDeviceID")]
    pnp_device_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PhysicalDisk {
    device_id: Option<String>,
    friendly_name: Option<String>,
    media_type: Option<u16>,
    bus_type: Option<u16>,
    size: Option<u64>,
    health_status: Option<u16>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Partition {
    disk_number: Option<u32>,
    /// CIM Char16; partitions without a letter report '\0'. Read as a raw Variant
    /// because its COM representation varies (UI2 / I2 / String).
    drive_letter: Option<wmi::Variant>,
}

fn variant_letter(v: &wmi::Variant) -> Option<char> {
    let c = match v {
        wmi::Variant::UI2(u) => char::from_u32(*u as u32),
        wmi::Variant::I2(i) => char::from_u32(*i as u16 as u32),
        wmi::Variant::UI1(u) => char::from_u32(*u as u32),
        wmi::Variant::String(s) => s.chars().next(),
        _ => None,
    }?;
    c.is_ascii_alphabetic().then(|| c.to_ascii_uppercase())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LogicalDisk {
    #[serde(rename = "DeviceID")]
    device_id: Option<String>,
    size: Option<u64>,
    free_space: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct BaseBoard {
    manufacturer: Option<String>,
    product: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Bios {
    #[serde(rename = "SMBIOSBIOSVersion")]
    smbios_bios_version: Option<String>,
    release_date: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Enclosure {
    chassis_types: Option<Vec<u16>>,
}

impl HardwareSource for WinHardwareSource {
    fn cpu(&self) -> SourceResult<RawCpu> {
        let rows: Vec<Processor> = self.query("cpu", "SELECT Name, Manufacturer, NumberOfCores, NumberOfLogicalProcessors, MaxClockSpeed FROM Win32_Processor")?;
        let first = rows
            .first()
            .ok_or_else(|| SourceError::new("cpu", "no Win32_Processor rows"))?;
        Ok(RawCpu {
            name: first.name.clone().unwrap_or_default(),
            manufacturer: first.manufacturer.clone().unwrap_or_default(),
            cores: rows.iter().filter_map(|r| r.number_of_cores).sum(),
            threads: rows
                .iter()
                .filter_map(|r| r.number_of_logical_processors)
                .sum(),
            max_clock_mhz: first.max_clock_speed,
            efficiency_classes: sys::core_efficiency_classes(),
        })
    }

    fn memory(&self) -> SourceResult<RawMemory> {
        let mods: Vec<PhysicalMemory> = self.query(
            "memory",
            "SELECT Capacity, Speed, ConfiguredClockSpeed, DeviceLocator, BankLabel, PartNumber, Manufacturer, SMBIOSMemoryType FROM Win32_PhysicalMemory",
        )?;
        let arrays: Vec<MemoryArray> = self
            .query(
                "memory",
                "SELECT MemoryDevices, Use FROM Win32_PhysicalMemoryArray",
            )
            .unwrap_or_default();
        let cs: Vec<ComputerSystem> = self
            .query(
                "memory",
                "SELECT TotalPhysicalMemory FROM Win32_ComputerSystem",
            )
            .unwrap_or_default();
        Ok(RawMemory {
            modules: mods
                .into_iter()
                .map(|m| RawMemoryModule {
                    capacity_bytes: m.capacity.unwrap_or(0),
                    speed_mts: m.speed.filter(|&s| s > 0),
                    configured_mts: m.configured_clock_speed.filter(|&s| s > 0),
                    device_locator: m.device_locator.unwrap_or_default(),
                    bank_label: m.bank_label.unwrap_or_default(),
                    part_number: m.part_number.unwrap_or_default(),
                    manufacturer: m.manufacturer.unwrap_or_default(),
                    smbios_type: m.smbios_memory_type.filter(|&t| t > 2),
                })
                .collect(),
            total_visible_bytes: cs
                .first()
                .and_then(|c| c.total_physical_memory)
                .unwrap_or(0),
            // Use = 3: system memory
            slots_total: Some(
                arrays
                    .iter()
                    .filter(|a| a.use_ == Some(3))
                    .filter_map(|a| a.memory_devices)
                    .sum::<u32>(),
            )
            .filter(|&n| n > 0),
        })
    }

    fn gpus(&self) -> SourceResult<Vec<RawGpu>> {
        let adapters =
            sys::dxgi_adapters().map_err(|e| SourceError::new("gpus", format!("DXGI: {e}")))?;
        let vcs: Vec<VideoController> = self
            .query(
                "gpus",
                "SELECT Name, DriverVersion, DriverDate, PNPDeviceID FROM Win32_VideoController",
            )
            .unwrap_or_default();
        Ok(adapters
            .into_iter()
            .map(|a| {
                // Match WMI controller by PCI vendor/device id, falling back to the name.
                let vc = vcs
                    .iter()
                    .find(|v| {
                        v.pnp_device_id.as_deref().and_then(pci_ids)
                            == Some((a.vendor_id, a.device_id))
                    })
                    .or_else(|| {
                        vcs.iter()
                            .find(|v| v.name.as_deref() == Some(a.name.as_str()))
                    });
                let pnp = vc.and_then(|v| v.pnp_device_id.clone());
                RawGpu {
                    pcie: pnp
                        .as_deref()
                        .filter(|p| p.starts_with("PCI\\"))
                        .and_then(sys::pcie_link),
                    name: a.name,
                    vendor_id: a.vendor_id,
                    device_id: a.device_id,
                    dedicated_bytes: a.dedicated_bytes,
                    shared_bytes: a.shared_bytes,
                    driver_version: vc.and_then(|v| v.driver_version.clone()),
                    driver_date: vc.and_then(|v| v.driver_date.as_deref()).and_then(cim_date),
                    pnp_device_id: pnp,
                }
            })
            .collect())
    }

    fn drives(&self) -> SourceResult<Vec<RawDrive>> {
        let st = self
            .storage
            .as_ref()
            .map_err(|e| SourceError::new("drives", format!("WMI Storage: {e}")))?;
        let disks: Vec<PhysicalDisk> =
            st.raw_query("SELECT DeviceId, FriendlyName, MediaType, BusType, Size, HealthStatus FROM MSFT_PhysicalDisk").map_err(err("drives"))?;
        // Partitions/volumes are required to know which disk holds the system: don't swallow errors.
        let parts: Vec<Partition> = st
            .raw_query("SELECT DiskNumber, DriveLetter FROM MSFT_Partition")
            .map_err(err("drives"))?;
        let logical: Vec<LogicalDisk> = self.query(
            "drives",
            "SELECT DeviceID, Size, FreeSpace FROM Win32_LogicalDisk WHERE DriveType = 3",
        )?;
        let space: HashMap<String, (u64, u64)> = logical
            .into_iter()
            .filter_map(|l| {
                Some((
                    l.device_id?.to_ascii_uppercase(),
                    (l.size.unwrap_or(0), l.free_space.unwrap_or(0)),
                ))
            })
            .collect();
        Ok(disks
            .into_iter()
            // USB sticks/external drives are not where games or Windows live: leave them out.
            .filter(|d| d.bus_type != Some(7))
            .map(|d| {
                let num: Option<u32> = d.device_id.as_deref().and_then(|s| s.parse().ok());
                let volumes = parts
                    .iter()
                    .filter(|p| p.disk_number == num && num.is_some())
                    .filter_map(|p| p.drive_letter.as_ref().and_then(variant_letter))
                    .filter_map(|c| {
                        let letter = format!("{c}:");
                        let (size, free) = *space.get(&letter)?;
                        Some(RawVolume {
                            letter,
                            size_bytes: size,
                            free_bytes: free,
                        })
                    })
                    .collect();
                RawDrive {
                    model: d.friendly_name.unwrap_or_default(),
                    media: match d.media_type {
                        Some(3) => RawMedia::Hdd,
                        Some(4) | Some(5) => RawMedia::Ssd,
                        _ => RawMedia::Unknown,
                    },
                    bus: match d.bus_type {
                        Some(17) => RawBus::Nvme,
                        Some(11) => RawBus::Sata,
                        Some(7) => RawBus::Usb,
                        _ => RawBus::Other,
                    },
                    size_bytes: d.size.unwrap_or(0),
                    health: match d.health_status {
                        Some(0) => RawHealth::Healthy,
                        Some(1) => RawHealth::Warning,
                        Some(2) => RawHealth::Unhealthy,
                        _ => RawHealth::Unknown,
                    },
                    volumes,
                }
            })
            .collect())
    }

    fn system_drive_letter(&self) -> String {
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
    }

    fn power(&self) -> SourceResult<RawPower> {
        let (ac, battery) = sys::power_status()
            .ok_or_else(|| SourceError::new("power", "GetSystemPowerStatus failed"))?;
        let enc: Vec<Enclosure> = self
            .query("power", "SELECT ChassisTypes FROM Win32_SystemEnclosure")
            .unwrap_or_default();
        // SMBIOS chassis: 8 Portable, 9 Laptop, 10 Notebook, 11 Hand Held, 12 Docking, 14 Sub Notebook,
        // 18 Expansion, 21 Peripheral, 30 Tablet, 31 Convertible, 32 Detachable
        let portable = enc
            .iter()
            .filter_map(|e| e.chassis_types.as_ref())
            .flatten()
            .map(|t| matches!(t, 8 | 9 | 10 | 11 | 14 | 30 | 31 | 32))
            .reduce(|a, b| a || b);
        Ok(RawPower {
            ac_online: ac,
            has_battery: battery,
            chassis_is_portable: portable,
            active_scheme_guid: sys::active_power_scheme(),
        })
    }

    fn os(&self) -> SourceResult<RawOs> {
        const K: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
        let h = &self.hive;
        let build = reg_string(Root::Machine, h, K, "CurrentBuildNumber")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| SourceError::new("os", "CurrentBuildNumber not readable"))?;
        Ok(RawOs {
            product_name: reg_string(Root::Machine, h, K, "ProductName").unwrap_or_default(),
            build,
            display_version: reg_string(Root::Machine, h, K, "DisplayVersion"),
        })
    }

    fn windows_settings(&self) -> SourceResult<RawWindowsSettings> {
        let h = &self.hive;
        // Game Mode: on by default since 1703; explicit 0 turns it off.
        let game_mode = Some(
            reg_dword(
                Root::User,
                h,
                "Software\\Microsoft\\GameBar",
                "AutoGameModeEnabled",
            ) != Some(0),
        );
        // Game DVR capture: both switches default to on when absent.
        let dvr = reg_dword(Root::User, h, "System\\GameConfigStore", "GameDVR_Enabled")
            .unwrap_or(1)
            != 0;
        let capture = reg_dword(
            Root::User,
            h,
            "Software\\Microsoft\\Windows\\CurrentVersion\\GameDVR",
            "AppCaptureEnabled",
        )
        .unwrap_or(1)
            != 0;
        let policy_off = reg_dword(
            Root::Machine,
            h,
            "SOFTWARE\\Policies\\Microsoft\\Windows\\GameDVR",
            "AllowGameDVR",
        ) == Some(0);
        let hags = reg_dword(
            Root::Machine,
            h,
            "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
            "HwSchMode",
        )
        .map(|v| v == 2);
        let dx = reg_string(
            Root::User,
            h,
            "Software\\Microsoft\\DirectX\\UserGpuPreferences",
            "DirectXUserGlobalSettings",
        )
        .unwrap_or_default();
        let flag = |name: &str| -> Option<bool> {
            dx.split(';').find_map(|kv| {
                kv.trim()
                    .strip_prefix(name)
                    .and_then(|v| v.strip_prefix('='))
                    .map(|v| v.trim() == "1")
            })
        };
        let pending = reg_key_exists(Root::Machine, h, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired")
            || reg_key_exists(Root::Machine, h, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending");
        Ok(RawWindowsSettings {
            game_mode,
            hags,
            game_dvr: Some(dvr && capture && !policy_off),
            windowed_optimizations: flag("SwapEffectUpgradeEnable"),
            vrr: flag("VRROptimizeEnable"),
            pending_reboot: Some(pending),
        })
    }

    fn firmware(&self) -> SourceResult<RawFirmware> {
        let bb: Vec<BaseBoard> = self.query(
            "firmware",
            "SELECT Manufacturer, Product FROM Win32_BaseBoard",
        )?;
        let bios: Vec<Bios> = self
            .query(
                "firmware",
                "SELECT SMBIOSBIOSVersion, ReleaseDate FROM Win32_BIOS",
            )
            .unwrap_or_default();
        let b = bb.into_iter().next();
        let bi = bios.into_iter().next();
        Ok(RawFirmware {
            board_manufacturer: b
                .as_ref()
                .and_then(|x| x.manufacturer.clone())
                .unwrap_or_default(),
            board_product: b.and_then(|x| x.product).unwrap_or_default(),
            bios_version: bi
                .as_ref()
                .and_then(|x| x.smbios_bios_version.clone())
                .unwrap_or_default(),
            bios_date: bi
                .and_then(|x| x.release_date)
                .as_deref()
                .and_then(cim_date),
        })
    }

    fn perf_sample(&self) -> SourceResult<RawPerfSample> {
        let rx = self
            .perf
            .borrow_mut()
            .take()
            .ok_or_else(|| SourceError::new("perf_sample", "already consumed"))?;
        rx.recv_timeout(PERF_WINDOW + Duration::from_secs(2))
            .map_err(|_| SourceError::new("perf_sample", "PDH sample timed out"))?
            .map_err(|e| SourceError::new("perf_sample", e))
    }

    fn today(&self) -> String {
        let days = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() / 86_400)
            .unwrap_or(0) as i64;
        civil_from_days(days)
    }
}

impl StabilitySource for WinHardwareSource {
    fn stability_events(&self, since_unix_ms: u64) -> SourceResult<RawStabilityEvents> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        sys::stability_events(now.saturating_sub(since_unix_ms))
            .map_err(|e| SourceError::new("stability", e))
    }
}

/// Days since 1970-01-01 → "YYYY-MM-DD" (H. Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(
            cim_date("20250901000000.000000-000").as_deref(),
            Some("2025-09-01")
        );
        assert_eq!(cim_date("garbage"), None);
        assert_eq!(
            pci_ids("PCI\\VEN_10DE&DEV_1F99&SUBSYS_3A4917AA&REV_A1\\4&1"),
            Some((0x10DE, 0x1F99))
        );
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(
            civil_from_days(days_from_iso("2026-11-15").unwrap()),
            "2026-11-15"
        );
    }
}
