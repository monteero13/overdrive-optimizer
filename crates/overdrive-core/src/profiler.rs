//! Raw facts → `HardwareProfile`. Each section is built independently: if a source
//! fails, that section is `unknown` and the error is recorded in `warnings`.

use crate::memspeed;
use crate::profile::*;
use crate::source::*;
use crate::tier;

const MIB: f64 = 1024.0 * 1024.0;
const GIB: f64 = MIB * 1024.0;

pub fn build_profile(src: &dyn HardwareSource) -> HardwareProfile {
    let mut warnings = Vec::new();
    fn take<T>(w: &mut Vec<String>, r: SourceResult<T>) -> Option<T> {
        r.map_err(|e| w.push(e.to_string())).ok()
    }

    let raw_cpu = take(&mut warnings, src.cpu());
    let raw_perf = take(&mut warnings, src.perf_sample());
    let raw_mem = take(&mut warnings, src.memory());
    let raw_gpus = take(&mut warnings, src.gpus());
    let raw_drives = take(&mut warnings, src.drives());
    let raw_power = take(&mut warnings, src.power());
    let raw_os = take(&mut warnings, src.os());
    let raw_win = take(&mut warnings, src.windows_settings());
    let raw_fw = take(&mut warnings, src.firmware());
    let today = src.today();

    let firmware = firmware(raw_fw.as_ref());
    let cpu = cpu(raw_cpu.as_ref(), raw_perf.as_ref());
    let power = power(raw_power.as_ref());
    let memory = memory(raw_mem.as_ref(), raw_cpu.as_ref(), &firmware, &power);
    let gpus = raw_gpus
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|g| gpu(g, &today))
        .collect();
    let storage = drives(
        raw_drives.as_deref().unwrap_or_default(),
        &src.system_drive_letter(),
    );
    let windows = windows(raw_os.as_ref(), raw_win.as_ref());

    let mut p = HardwareProfile {
        schema_version: SCHEMA_VERSION,
        cpu,
        memory,
        gpus,
        storage,
        power,
        windows,
        firmware,
        tier: Tier::Tier2,
        warnings,
    };
    p.tier = tier::classify(&p);
    p
}

// ---------------------------------------------------------------- CPU
fn cpu(raw: Option<&RawCpu>, perf: Option<&RawPerfSample>) -> CpuInfo {
    const S: &str = "WMI Win32_Processor";
    const T: &str = "GetLogicalProcessorInformationEx";
    let Some(c) = raw else {
        return CpuInfo {
            name: Field::unknown(S),
            vendor: Field::unknown(S),
            cores: Field::unknown(S),
            threads: Field::unknown(S),
            performance_cores: Field::unknown(T),
            efficiency_cores: Field::unknown(T),
            max_clock_mhz: Field::unknown(S),
            generation: Field::unknown(S),
            perf_limit_pct: perf_limit(perf).0,
            throttling: perf_limit(perf).1,
        };
    };
    let vendor = if c.manufacturer.contains("Intel") || c.name.contains("Intel") {
        CpuVendor::Intel
    } else if c.manufacturer.contains("AMD") || c.name.contains("AMD") {
        CpuVendor::Amd
    } else {
        CpuVendor::Other
    };
    let (p_cores, e_cores) = match &c.efficiency_classes {
        Some(classes) if !classes.is_empty() => {
            let max = *classes.iter().max().unwrap();
            let min = *classes.iter().min().unwrap();
            if max == min {
                (Field::exact(classes.len() as u32, T), Field::exact(0, T))
            } else {
                let p = classes.iter().filter(|&&x| x == max).count() as u32;
                (
                    Field::exact(p, T),
                    Field::exact(classes.len() as u32 - p, T),
                )
            }
        }
        _ => (Field::unknown(T), Field::unknown(T)),
    };
    let generation = match vendor {
        CpuVendor::Intel => memspeed::intel_core_model(&c.name).map(|m| m.0),
        CpuVendor::Amd => memspeed::ryzen_series(&c.name),
        CpuVendor::Other => None,
    };
    let (perf_limit_pct, throttling) = perf_limit(perf);
    CpuInfo {
        name: Field::exact(c.name.trim().to_string(), S),
        vendor: Field::exact(vendor, S),
        cores: Field::from_option((c.cores > 0).then_some(c.cores), Confidence::Exact, S),
        threads: Field::from_option((c.threads > 0).then_some(c.threads), Confidence::Exact, S),
        performance_cores: p_cores,
        efficiency_cores: e_cores,
        max_clock_mhz: Field::from_option(c.max_clock_mhz, Confidence::Exact, S),
        generation: Field::from_option(generation, Confidence::Inferred, "CPU brand string"),
        perf_limit_pct,
        throttling,
    }
}

fn perf_limit(perf: Option<&RawPerfSample>) -> (Field<f64>, Field<bool>) {
    const P: &str = "PDH \\Processor Information(_Total)\\% Performance Limit";
    match perf.and_then(|p| p.perf_limit_pct) {
        // A single short sample: label the throttling verdict as inferred.
        Some(v) => (Field::exact(v, P), Field::inferred(v < 85.0, P)),
        None => (Field::unknown(P), Field::unknown(P)),
    }
}

// ---------------------------------------------------------------- Memory
fn memory(
    raw: Option<&RawMemory>,
    cpu: Option<&RawCpu>,
    fw: &FirmwareInfo,
    power: &PowerContext,
) -> MemoryInfo {
    const S: &str = "WMI Win32_PhysicalMemory (SMBIOS)";
    const PN: &str = "part number";
    let Some(m) = raw else {
        return MemoryInfo {
            total_gb: Field::unknown(S),
            kind: Field::unknown(S),
            modules: vec![],
            slots_total: Field::unknown(S),
            slots_used: Field::unknown(S),
            configured_mts: Field::unknown(S),
            rated_mts: Field::unknown(S),
            platform_cap_mts: Field::unknown("CPU + chipset"),
            channels: Field::unknown(S),
        };
    };
    let modules: Vec<MemoryModuleInfo> = m
        .modules
        .iter()
        .map(|d| {
            let pn = memspeed::rated_from_part_number(&d.part_number);
            let jedec = d.speed_mts.map(memspeed::snap);
            let rated = match (pn, jedec) {
                (Some(p), Some(j)) if p > j => Field::inferred(p, PN),
                (Some(p), None) => Field::inferred(p, PN),
                (_, Some(j)) => Field::exact(j, S),
                (None, None) => Field::unknown(S),
            };
            MemoryModuleInfo {
                capacity_gb: round1(d.capacity_bytes as f64 / GIB),
                locator: d.device_locator.trim().to_string(),
                part_number: d.part_number.trim().to_string(),
                manufacturer: d.manufacturer.trim().to_string(),
                jedec_max_mts: jedec,
                configured_mts: d.configured_mts.map(memspeed::snap),
                rated_mts: rated,
            }
        })
        .collect();

    let configured = modules.iter().filter_map(|x| x.configured_mts).min();
    // Kit rating = the slowest module's rating (mixed kits run at the slowest).
    let rated = modules
        .iter()
        .filter_map(|x| x.rated_mts.val().map(|v| (v, x.rated_mts.confidence)))
        .min_by_key(|(v, _)| *v);
    let kind = m
        .modules
        .iter()
        .find_map(|d| d.smbios_type)
        .map(|t| match t {
            24 => MemoryKind::Ddr3,
            26 => MemoryKind::Ddr4,
            34 | 35 => MemoryKind::Ddr5,
            _ => MemoryKind::Other,
        });
    let locs: Vec<(String, String)> = m
        .modules
        .iter()
        .map(|d| (d.device_locator.clone(), d.bank_label.clone()))
        .collect();
    let channels = memspeed::infer_channels(&locs);

    // Platform cap (Intel desktop on a non-overclocking chipset). Laptops: not claimed here.
    let cap = (|| {
        if power.is_laptop.val() == Some(true) {
            return None;
        }
        let c = cpu?;
        let (gen, tier, suffix) = memspeed::intel_core_model(&c.name)?;
        memspeed::intel_platform_cap(gen, tier, &suffix, fw.chipset.get().map(String::as_str))
    })();

    let total = if m.total_visible_bytes > 0 {
        m.total_visible_bytes as f64
    } else {
        m.modules.iter().map(|d| d.capacity_bytes as f64).sum()
    };
    MemoryInfo {
        total_gb: Field::from_option(
            (total > 0.0).then_some((total / GIB * 10.0).round() / 10.0),
            Confidence::Exact,
            S,
        ),
        kind: Field::from_option(kind, Confidence::Exact, S),
        slots_used: Field::exact(modules.len() as u32, S),
        modules,
        slots_total: Field::from_option(
            m.slots_total,
            Confidence::Exact,
            "WMI Win32_PhysicalMemoryArray",
        ),
        configured_mts: Field::from_option(configured, Confidence::Exact, S),
        rated_mts: match rated {
            Some((v, c)) => Field {
                value: Some(v),
                confidence: c,
                source: if c == Confidence::Inferred {
                    PN.into()
                } else {
                    S.into()
                },
            },
            None => Field::unknown(S),
        },
        platform_cap_mts: Field::from_option(cap, Confidence::Inferred, "CPU generation + chipset"),
        channels: Field::from_option(channels, Confidence::Inferred, "DIMM locators"),
    }
}

// ---------------------------------------------------------------- GPU
fn gpu(g: &RawGpu, today: &str) -> Option<GpuInfo> {
    const S: &str = "DXGI";
    const W: &str = "WMI Win32_VideoController";
    if g.vendor_id == 0x1414 {
        return None; // Microsoft Basic Render Driver / software adapters
    }
    let vendor = match g.vendor_id {
        0x10DE => GpuVendor::Nvidia,
        0x1002 | 0x1022 => GpuVendor::Amd,
        0x8086 => GpuVendor::Intel,
        _ => GpuVendor::Other,
    };
    let ded_mb = (g.dedicated_bytes as f64 / MIB).round() as u32;
    let name_up = g.name.to_ascii_uppercase();
    let kind = match vendor {
        GpuVendor::Nvidia => Field::exact(GpuKind::Dedicated, "vendor"),
        GpuVendor::Intel if name_up.contains("ARC") && ded_mb >= 2048 => {
            Field::inferred(GpuKind::Dedicated, "name + VRAM")
        }
        GpuVendor::Intel => Field::inferred(GpuKind::Integrated, "vendor + VRAM"),
        _ if ded_mb >= 1024 => Field::inferred(GpuKind::Dedicated, "VRAM ≥ 1 GB"),
        _ => Field::inferred(GpuKind::Integrated, "VRAM < 1 GB"),
    };
    let age = g
        .driver_date
        .as_deref()
        .and_then(|d| days_between(d, today));
    Some(GpuInfo {
        name: g.name.trim().to_string(),
        vendor,
        kind,
        vram_mb: Field::exact(ded_mb, S),
        shared_mb: Field::exact((g.shared_bytes as f64 / MIB).round() as u32, S),
        driver_version: Field::from_option(g.driver_version.clone(), Confidence::Exact, W),
        driver_date: Field::from_option(g.driver_date.clone(), Confidence::Exact, W),
        driver_age_days: Field::from_option(age, Confidence::Exact, W),
        pcie: Field::from_option(
            g.pcie.map(|l| PcieLink {
                current_width: l.current_width,
                max_width: l.max_width,
                current_gen: l.current_gen,
                max_gen: l.max_gen,
            }),
            Confidence::Exact,
            "cfgmgr32 DEVPKEY_PciDevice_*",
        ),
    })
}

// ---------------------------------------------------------------- Storage
fn drives(raw: &[RawDrive], system_letter: &str) -> Vec<DriveInfo> {
    const S: &str = "WMI MSFT_PhysicalDisk";
    raw.iter()
        .map(|d| {
            let media = match (d.media, d.bus) {
                (RawMedia::Hdd, _) => Field::exact(MediaType::Hdd, S),
                (RawMedia::Ssd, RawBus::Nvme) => Field::exact(MediaType::NvmeSsd, S),
                (RawMedia::Ssd, RawBus::Sata) => Field::exact(MediaType::SataSsd, S),
                (RawMedia::Ssd, _) => Field::inferred(MediaType::SataSsd, S),
                (RawMedia::Unknown, RawBus::Nvme) => {
                    Field::inferred(MediaType::NvmeSsd, "bus type")
                }
                (RawMedia::Unknown, _) => Field::unknown(S),
            };
            let health = match d.health {
                RawHealth::Healthy => Field::exact(DriveHealth::Healthy, S),
                RawHealth::Warning => Field::exact(DriveHealth::Warning, S),
                RawHealth::Unhealthy => Field::exact(DriveHealth::Unhealthy, S),
                RawHealth::Unknown => Field::unknown(S),
            };
            let volumes: Vec<VolumeInfo> = d
                .volumes
                .iter()
                .filter(|v| v.size_bytes > 0)
                .map(|v| VolumeInfo {
                    letter: v.letter.trim_end_matches('\\').to_ascii_uppercase(),
                    size_gb: round1(v.size_bytes as f64 / GIB),
                    free_pct: ((v.free_bytes as f64 / v.size_bytes as f64) * 1000.0).round() / 10.0,
                })
                .collect();
            let is_system = volumes.iter().any(|v| {
                v.letter
                    .eq_ignore_ascii_case(system_letter.trim_end_matches('\\'))
            });
            DriveInfo {
                model: d.model.trim().to_string(),
                media,
                size_gb: round1(d.size_bytes as f64 / GIB),
                health,
                volumes,
                is_system,
            }
        })
        .collect()
}

// ---------------------------------------------------------------- Power / OS / firmware
fn power(raw: Option<&RawPower>) -> PowerContext {
    const S: &str = "GetSystemPowerStatus + SMBIOS chassis";
    let Some(p) = raw else {
        return PowerContext {
            is_laptop: Field::unknown(S),
            power_source: Field::unknown(S),
            active_scheme_guid: Field::unknown("PowerGetActiveScheme"),
        };
    };
    let is_laptop = match (p.chassis_is_portable, p.has_battery) {
        (Some(c), _) => Field::exact(c, "SMBIOS chassis"),
        (None, true) => Field::inferred(true, "battery present"),
        (None, false) => Field::inferred(false, "no battery"),
    };
    PowerContext {
        is_laptop,
        power_source: Field::from_option(
            p.ac_online.map(|ac| {
                if ac {
                    PowerSource::Ac
                } else {
                    PowerSource::Battery
                }
            }),
            Confidence::Exact,
            S,
        ),
        active_scheme_guid: Field::from_option(
            p.active_scheme_guid.clone(),
            Confidence::Exact,
            "PowerGetActiveScheme",
        ),
    }
}

fn windows(os: Option<&RawOs>, w: Option<&RawWindowsSettings>) -> WindowsInfo {
    const R: &str = "registry";
    let (name, build, w11) = match os {
        Some(o) => {
            // ProductName still says "Windows 10" on Windows 11: the build number is the truth.
            let w11 = o.build >= 22000;
            let base = if w11 { "Windows 11" } else { "Windows 10" };
            let edition = o
                .product_name
                .split_whitespace()
                .skip(2)
                .collect::<Vec<_>>()
                .join(" ");
            let name = [
                base.to_string(),
                edition,
                o.display_version.clone().unwrap_or_default(),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            (
                Field::exact(name, R),
                Field::exact(o.build, R),
                Field::exact(w11, "build ≥ 22000"),
            )
        }
        None => (Field::unknown(R), Field::unknown(R), Field::unknown(R)),
    };
    let f = |v: Option<bool>| Field::from_option(v, Confidence::Exact, R);
    let w = w.cloned().unwrap_or_default();
    WindowsInfo {
        name,
        build,
        is_windows_11: w11,
        game_mode: f(w.game_mode),
        hags: f(w.hags),
        game_dvr: f(w.game_dvr),
        windowed_optimizations: f(w.windowed_optimizations),
        vrr: f(w.vrr),
        pending_reboot: f(w.pending_reboot),
    }
}

pub fn board_vendor(manufacturer: &str) -> BoardVendor {
    let m = manufacturer.to_ascii_lowercase();
    if m.contains("micro-star") || m.contains("msi") {
        BoardVendor::Msi
    } else if m.contains("asus") {
        BoardVendor::Asus
    } else if m.contains("gigabyte") {
        BoardVendor::Gigabyte
    } else if m.contains("asrock") {
        BoardVendor::Asrock
    } else if m.contains("lenovo") {
        BoardVendor::Lenovo
    } else if m.contains("hewlett") || m == "hp" || m.starts_with("hp ") {
        BoardVendor::Hp
    } else if m.contains("dell") {
        BoardVendor::Dell
    } else if m.contains("acer") {
        BoardVendor::Acer
    } else {
        BoardVendor::Generic
    }
}

fn firmware(raw: Option<&RawFirmware>) -> FirmwareInfo {
    const S: &str = "WMI Win32_BaseBoard / Win32_BIOS";
    let Some(f) = raw else {
        return FirmwareInfo {
            board_vendor: Field::unknown(S),
            board_manufacturer: Field::unknown(S),
            board_product: Field::unknown(S),
            chipset: Field::unknown(S),
            bios_version: Field::unknown(S),
            bios_date: Field::unknown(S),
        };
    };
    let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    FirmwareInfo {
        board_vendor: Field::exact(board_vendor(&f.board_manufacturer), S),
        board_manufacturer: Field::from_option(
            non_empty(&f.board_manufacturer),
            Confidence::Exact,
            S,
        ),
        board_product: Field::from_option(non_empty(&f.board_product), Confidence::Exact, S),
        chipset: Field::from_option(
            memspeed::chipset_from_board(&f.board_product),
            Confidence::Inferred,
            "board product name",
        ),
        bios_version: Field::from_option(non_empty(&f.bios_version), Confidence::Exact, S),
        bios_date: Field::from_option(f.bios_date.clone(), Confidence::Exact, S),
    }
}

// ---------------------------------------------------------------- dates
/// Days since 1970-01-01 for an ISO date (YYYY-MM-DD). Civil-from-days algorithm (H. Hinnant).
pub fn days_from_iso(d: &str) -> Option<i64> {
    let mut it = d.get(..10)?.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let day: i64 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

pub fn days_between(from: &str, to: &str) -> Option<u32> {
    let d = days_from_iso(to)? - days_from_iso(from)?;
    (d >= 0).then_some(d as u32)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(days_from_iso("1970-01-01"), Some(0));
        assert_eq!(days_between("2024-01-15", "2026-11-15"), Some(1035));
        assert_eq!(days_between("2026-11-16", "2026-11-15"), None);
        assert_eq!(days_from_iso("garbage"), None);
    }

    #[test]
    fn vendors() {
        assert_eq!(
            board_vendor("Micro-Star International Co., Ltd."),
            BoardVendor::Msi
        );
        assert_eq!(board_vendor("ASUSTeK COMPUTER INC."), BoardVendor::Asus);
        assert_eq!(board_vendor("LENOVO"), BoardVendor::Lenovo);
        assert_eq!(board_vendor("Hewlett-Packard"), BoardVendor::Hp);
        assert_eq!(board_vendor("Something"), BoardVendor::Generic);
    }
}
