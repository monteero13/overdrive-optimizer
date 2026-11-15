//! Raw facts as the OS reports them — minimal processing, no inference.
//! `overdrive-win` implements [`HardwareSource`] with WMI/DXGI/cfgmgr32/PDH/registry;
//! tests use [`crate::testing::FakeSource`].

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
#[error("{source_name}: {message}")]
pub struct SourceError {
    pub source_name: &'static str,
    pub message: String,
}

impl SourceError {
    pub fn new(source_name: &'static str, message: impl Into<String>) -> Self {
        Self {
            source_name,
            message: message.into(),
        }
    }
}

pub type SourceResult<T> = Result<T, SourceError>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawCpu {
    pub name: String,
    pub manufacturer: String,
    pub cores: u32,
    pub threads: u32,
    pub max_clock_mhz: Option<u32>,
    /// EfficiencyClass per physical core (GetLogicalProcessorInformationEx). Higher = performance core.
    pub efficiency_classes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawMemoryModule {
    pub capacity_bytes: u64,
    /// SMBIOS "Speed": max speed the BIOS reports for the DIMM (usually the JEDEC speed, NOT XMP).
    pub speed_mts: Option<u32>,
    /// SMBIOS "ConfiguredClockSpeed": what it runs at now.
    pub configured_mts: Option<u32>,
    pub device_locator: String,
    pub bank_label: String,
    pub part_number: String,
    pub manufacturer: String,
    /// SMBIOS memory type (26 = DDR4, 34 = DDR5, 24 = DDR3).
    pub smbios_type: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawMemory {
    pub modules: Vec<RawMemoryModule>,
    pub total_visible_bytes: u64,
    pub slots_total: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawPcieLink {
    pub current_width: Option<u32>,
    pub max_width: Option<u32>,
    /// PCI_EXPRESS_LINK_SPEED: 1 = Gen1 … 5 = Gen5. Drops at idle on most GPUs.
    pub current_gen: Option<u32>,
    pub max_gen: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawGpu {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_bytes: u64,
    pub shared_bytes: u64,
    pub driver_version: Option<String>,
    /// ISO date YYYY-MM-DD.
    pub driver_date: Option<String>,
    pub pnp_device_id: Option<String>,
    pub pcie: Option<RawPcieLink>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RawMedia {
    Hdd,
    Ssd,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RawBus {
    Nvme,
    Sata,
    Usb,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RawHealth {
    Healthy,
    Warning,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawVolume {
    pub letter: String,
    pub size_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDrive {
    pub model: String,
    pub media: RawMedia,
    pub bus: RawBus,
    pub size_bytes: u64,
    pub health: RawHealth,
    pub volumes: Vec<RawVolume>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawPower {
    pub ac_online: Option<bool>,
    pub has_battery: bool,
    /// From SMBIOS chassis types (laptop/notebook/convertible…).
    pub chassis_is_portable: Option<bool>,
    pub active_scheme_guid: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawOs {
    pub product_name: String,
    pub build: u32,
    pub display_version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawWindowsSettings {
    pub game_mode: Option<bool>,
    pub hags: Option<bool>,
    pub game_dvr: Option<bool>,
    pub windowed_optimizations: Option<bool>,
    pub vrr: Option<bool>,
    pub pending_reboot: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawFirmware {
    pub board_manufacturer: String,
    pub board_product: String,
    pub bios_version: String,
    /// ISO date YYYY-MM-DD.
    pub bios_date: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RawPerfSample {
    /// `\Processor Information(_Total)\% Performance Limit` — 100 = no limit.
    pub perf_limit_pct: Option<f64>,
    /// `\Processor Information(_Total)\% Processor Performance` — can exceed 100 with boost.
    pub processor_performance_pct: Option<f64>,
}

/// Counts from the System event log since a point in time (BIOS Coach stability watch).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawStabilityEvents {
    pub whea_errors: u32,
    pub unexpected_shutdowns: u32,
    pub bugchecks: u32,
}

/// Every method is independent: one failing source never prevents the others
/// (its fields end up `unknown` and the error goes to `HardwareProfile::warnings`).
pub trait HardwareSource {
    fn cpu(&self) -> SourceResult<RawCpu>;
    fn memory(&self) -> SourceResult<RawMemory>;
    fn gpus(&self) -> SourceResult<Vec<RawGpu>>;
    fn drives(&self) -> SourceResult<Vec<RawDrive>>;
    fn system_drive_letter(&self) -> String {
        "C:".into()
    }
    fn power(&self) -> SourceResult<RawPower>;
    fn os(&self) -> SourceResult<RawOs>;
    fn windows_settings(&self) -> SourceResult<RawWindowsSettings>;
    fn firmware(&self) -> SourceResult<RawFirmware>;
    fn perf_sample(&self) -> SourceResult<RawPerfSample>;
    /// Today's date (YYYY-MM-DD) — injected so driver-age logic is testable.
    fn today(&self) -> String;
}

/// Separate from [`HardwareSource`]: only used by the BIOS Coach stability watch.
pub trait StabilitySource {
    /// Events since `since_unix_ms`.
    fn stability_events(&self, since_unix_ms: u64) -> SourceResult<RawStabilityEvents>;
}
