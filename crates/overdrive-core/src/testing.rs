//! Fake sources for tests and for the UI mock engine. Presets mirror the three
//! machines in `ui/web/src/mock/presets.ts` (one per tier).

use std::cell::RefCell;
use std::collections::HashSet;

use crate::source::*;

#[derive(Debug, Clone)]
pub struct FakeSource {
    pub cpu: RawCpu,
    pub memory: RawMemory,
    pub gpus: Vec<RawGpu>,
    pub drives: Vec<RawDrive>,
    pub power: RawPower,
    pub os: RawOs,
    pub settings: RawWindowsSettings,
    pub firmware: RawFirmware,
    pub perf: RawPerfSample,
    pub today: String,
    pub stability: RawStabilityEvents,
    /// Names of sources that should fail ("cpu", "memory", "gpus", …).
    pub failing: HashSet<&'static str>,
    pub calls: RefCell<Vec<&'static str>>,
}

const GB: u64 = 1024 * 1024 * 1024;

fn module(
    gb: u64,
    speed: u32,
    configured: u32,
    locator: &str,
    part: &str,
    ddr: u32,
) -> RawMemoryModule {
    RawMemoryModule {
        capacity_bytes: gb * GB,
        speed_mts: Some(speed),
        configured_mts: Some(configured),
        device_locator: locator.into(),
        bank_label: String::new(),
        part_number: part.into(),
        manufacturer: String::new(),
        smbios_type: Some(ddr),
    }
}

fn drive(
    model: &str,
    media: RawMedia,
    bus: RawBus,
    gb: u64,
    letter: &str,
    free_gb: u64,
) -> RawDrive {
    RawDrive {
        model: model.into(),
        media,
        bus,
        size_bytes: gb * GB,
        health: RawHealth::Healthy,
        volumes: vec![RawVolume {
            letter: letter.into(),
            size_bytes: gb * GB,
            free_bytes: free_gb * GB,
        }],
    }
}

impl FakeSource {
    fn fail(&self, name: &'static str) -> SourceResult<()> {
        self.calls.borrow_mut().push(name);
        if self.failing.contains(name) {
            Err(SourceError::new(name, "simulated failure"))
        } else {
            Ok(())
        }
    }

    /// Lenovo laptop, Ryzen 5 5600H + GTX 1650 4 GB (Optimus), 8 GB single stick, NVMe. Tier 3? No: 4 GB VRAM + 8 GB → Tier 2.
    pub fn laptop_hybrid() -> Self {
        Self {
            cpu: RawCpu {
                name: "AMD Ryzen 5 5600H with Radeon Graphics".into(),
                manufacturer: "AuthenticAMD".into(),
                cores: 6,
                threads: 12,
                max_clock_mhz: Some(3301),
                efficiency_classes: Some(vec![0; 6]),
            },
            memory: RawMemory {
                modules: vec![module(8, 3200, 3200, "DIMM 0", "M471A1K43DB1-CWE", 26)],
                total_visible_bytes: 7 * GB + GB / 2,
                slots_total: Some(2),
            },
            gpus: vec![
                RawGpu {
                    name: "AMD Radeon(TM) Graphics".into(),
                    vendor_id: 0x1002,
                    device_id: 0x1638,
                    dedicated_bytes: 512 * 1024 * 1024,
                    shared_bytes: 3 * GB,
                    driver_version: Some("31.0.21001.45002".into()),
                    driver_date: Some("2025-06-10".into()),
                    pnp_device_id: None,
                    pcie: None,
                },
                RawGpu {
                    name: "NVIDIA GeForce GTX 1650".into(),
                    vendor_id: 0x10DE,
                    device_id: 0x1F99,
                    dedicated_bytes: 4 * GB,
                    shared_bytes: 3 * GB,
                    driver_version: Some("32.0.15.6094".into()),
                    driver_date: Some("2025-09-01".into()),
                    pnp_device_id: None,
                    pcie: Some(RawPcieLink {
                        current_width: Some(8),
                        max_width: Some(8),
                        current_gen: Some(1),
                        max_gen: Some(3),
                    }),
                },
            ],
            drives: vec![drive(
                "SAMSUNG MZVLQ512HBLU",
                RawMedia::Ssd,
                RawBus::Nvme,
                476,
                "C:",
                150,
            )],
            power: RawPower {
                ac_online: Some(true),
                has_battery: true,
                chassis_is_portable: Some(true),
                active_scheme_guid: Some("381b4222-f694-41f0-9685-ff5bb260df2e".into()),
            },
            os: RawOs {
                product_name: "Windows 10 Home".into(),
                build: 26100,
                display_version: Some("24H2".into()),
            },
            settings: RawWindowsSettings {
                game_mode: Some(true),
                hags: Some(false),
                game_dvr: Some(true),
                windowed_optimizations: Some(false),
                vrr: Some(false),
                pending_reboot: Some(false),
            },
            firmware: RawFirmware {
                board_manufacturer: "LENOVO".into(),
                board_product: "LNVNB161216".into(),
                bios_version: "H3CN32WW".into(),
                bios_date: Some("2023-03-01".into()),
            },
            perf: RawPerfSample {
                perf_limit_pct: Some(100.0),
                processor_performance_pct: Some(98.0),
            },
            today: "2026-11-15".into(),
            stability: RawStabilityEvents::default(),
            failing: HashSet::new(),
            calls: RefCell::new(vec![]),
        }
    }

    /// MSI B460 desktop, i3-10100F + GTX 1050 Ti, 2×8 GB Corsair 3200 running at 2133 (XMP off), SATA SSD 91 % full.
    pub fn desktop_entry() -> Self {
        Self {
            cpu: RawCpu {
                name: "Intel(R) Core(TM) i3-10100F CPU @ 3.60GHz".into(),
                manufacturer: "GenuineIntel".into(),
                cores: 4,
                threads: 8,
                max_clock_mhz: Some(3600),
                efficiency_classes: Some(vec![0; 4]),
            },
            memory: RawMemory {
                modules: vec![
                    module(8, 2133, 2133, "DIMM_A1", "CMK16GX4M2B3200C16", 26),
                    module(8, 2133, 2133, "DIMM_B1", "CMK16GX4M2B3200C16", 26),
                ],
                total_visible_bytes: 16 * GB,
                slots_total: Some(4),
            },
            gpus: vec![RawGpu {
                name: "NVIDIA GeForce GTX 1050 Ti".into(),
                vendor_id: 0x10DE,
                device_id: 0x1C82,
                dedicated_bytes: 4 * GB,
                shared_bytes: 8 * GB,
                driver_version: Some("31.0.15.3623".into()),
                driver_date: Some("2023-06-20".into()),
                pnp_device_id: None,
                pcie: Some(RawPcieLink {
                    current_width: Some(16),
                    max_width: Some(16),
                    current_gen: Some(1),
                    max_gen: Some(3),
                }),
            }],
            drives: vec![drive(
                "KINGSTON SA400S37480G",
                RawMedia::Ssd,
                RawBus::Sata,
                447,
                "C:",
                40,
            )],
            power: RawPower {
                ac_online: Some(true),
                has_battery: false,
                chassis_is_portable: Some(false),
                active_scheme_guid: Some("381b4222-f694-41f0-9685-ff5bb260df2e".into()),
            },
            os: RawOs {
                product_name: "Windows 10 Pro".into(),
                build: 19045,
                display_version: Some("22H2".into()),
            },
            settings: RawWindowsSettings {
                game_mode: Some(true),
                hags: Some(false),
                game_dvr: Some(false),
                windowed_optimizations: None,
                vrr: None,
                pending_reboot: Some(false),
            },
            firmware: RawFirmware {
                board_manufacturer: "Micro-Star International Co., Ltd.".into(),
                board_product: "Z490-A PRO (MS-7C75)".into(),
                bios_version: "2.80".into(),
                bios_date: Some("2022-05-10".into()),
            },
            perf: RawPerfSample {
                perf_limit_pct: Some(100.0),
                processor_performance_pct: Some(105.0),
            },
            today: "2026-11-15".into(),
            stability: RawStabilityEvents::default(),
            failing: HashSet::new(),
            calls: RefCell::new(vec![]),
        }
    }

    /// ASUS A320 desktop, Ryzen 3 3200G (Vega 8, 512 MB UMA), 2×4 GB at 2400 of 3000, system on HDD.
    pub fn apu_lowend() -> Self {
        Self {
            cpu: RawCpu {
                name: "AMD Ryzen 3 3200G with Radeon Vega Graphics".into(),
                manufacturer: "AuthenticAMD".into(),
                cores: 4,
                threads: 4,
                max_clock_mhz: Some(3600),
                efficiency_classes: Some(vec![0; 4]),
            },
            memory: RawMemory {
                modules: vec![
                    module(4, 2400, 2400, "DIMM_A1", "F4-3000C16-4GISB", 26),
                    module(4, 2400, 2400, "DIMM_B1", "F4-3000C16-4GISB", 26),
                ],
                total_visible_bytes: 7 * GB + GB / 2,
                slots_total: Some(2),
            },
            gpus: vec![RawGpu {
                name: "AMD Radeon(TM) Vega 8 Graphics".into(),
                vendor_id: 0x1002,
                device_id: 0x15D8,
                dedicated_bytes: 512 * 1024 * 1024,
                shared_bytes: 3 * GB,
                driver_version: Some("31.0.12044.3".into()),
                driver_date: Some("2024-01-15".into()),
                pnp_device_id: None,
                pcie: None,
            }],
            drives: vec![drive(
                "ST1000DM010-2EP102",
                RawMedia::Hdd,
                RawBus::Sata,
                931,
                "C:",
                410,
            )],
            power: RawPower {
                ac_online: Some(true),
                has_battery: false,
                chassis_is_portable: Some(false),
                active_scheme_guid: None,
            },
            os: RawOs {
                product_name: "Windows 10 Home".into(),
                build: 19045,
                display_version: Some("22H2".into()),
            },
            settings: RawWindowsSettings {
                game_mode: Some(false),
                hags: None,
                game_dvr: Some(true),
                windowed_optimizations: None,
                vrr: None,
                pending_reboot: Some(true),
            },
            firmware: RawFirmware {
                board_manufacturer: "ASUSTeK COMPUTER INC.".into(),
                board_product: "PRIME A320M-K".into(),
                bios_version: "6042".into(),
                bios_date: Some("2021-12-01".into()),
            },
            perf: RawPerfSample {
                perf_limit_pct: Some(100.0),
                processor_performance_pct: Some(100.0),
            },
            today: "2026-11-15".into(),
            stability: RawStabilityEvents::default(),
            failing: HashSet::new(),
            calls: RefCell::new(vec![]),
        }
    }
}

impl HardwareSource for FakeSource {
    fn cpu(&self) -> SourceResult<RawCpu> {
        self.fail("cpu").map(|_| self.cpu.clone())
    }
    fn memory(&self) -> SourceResult<RawMemory> {
        self.fail("memory").map(|_| self.memory.clone())
    }
    fn gpus(&self) -> SourceResult<Vec<RawGpu>> {
        self.fail("gpus").map(|_| self.gpus.clone())
    }
    fn drives(&self) -> SourceResult<Vec<RawDrive>> {
        self.fail("drives").map(|_| self.drives.clone())
    }
    fn power(&self) -> SourceResult<RawPower> {
        self.fail("power").map(|_| self.power.clone())
    }
    fn os(&self) -> SourceResult<RawOs> {
        self.fail("os").map(|_| self.os.clone())
    }
    fn windows_settings(&self) -> SourceResult<RawWindowsSettings> {
        self.fail("windows_settings").map(|_| self.settings.clone())
    }
    fn firmware(&self) -> SourceResult<RawFirmware> {
        self.fail("firmware").map(|_| self.firmware.clone())
    }
    fn perf_sample(&self) -> SourceResult<RawPerfSample> {
        self.fail("perf_sample").map(|_| self.perf)
    }
    fn today(&self) -> String {
        self.today.clone()
    }
}

impl StabilitySource for FakeSource {
    fn stability_events(&self, _since_unix_ms: u64) -> SourceResult<RawStabilityEvents> {
        self.fail("stability").map(|_| self.stability)
    }
}
