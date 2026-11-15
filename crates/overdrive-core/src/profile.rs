//! `HardwareProfile` — the profiler's output (goals.md §5.1). Every fact carries its
//! confidence and data source; inferred data is never presented as fact.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Exact,
    Inferred,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Field<T> {
    pub value: Option<T>,
    pub confidence: Confidence,
    pub source: String,
}

impl<T> Field<T> {
    pub fn exact(value: T, source: &str) -> Self {
        Self {
            value: Some(value),
            confidence: Confidence::Exact,
            source: source.into(),
        }
    }
    pub fn inferred(value: T, source: &str) -> Self {
        Self {
            value: Some(value),
            confidence: Confidence::Inferred,
            source: source.into(),
        }
    }
    pub fn unknown(source: &str) -> Self {
        Self {
            value: None,
            confidence: Confidence::Unknown,
            source: source.into(),
        }
    }
    pub fn from_option(value: Option<T>, confidence: Confidence, source: &str) -> Self {
        match value {
            Some(v) => Self {
                value: Some(v),
                confidence,
                source: source.into(),
            },
            None => Self::unknown(source),
        }
    }
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }
    pub fn is_known(&self) -> bool {
        self.value.is_some()
    }
}

impl<T: Copy> Field<T> {
    pub fn val(&self) -> Option<T> {
        self.value
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// iGPU only, typically ≤ 8 GB RAM, ≤ 4 cores, often HDD.
    Tier1,
    /// Entry dedicated GPU (≤ 4 GB VRAM) or 8 GB RAM or ≤ 4 cores.
    Tier2,
    /// Mid-range parts, possibly held back by one weak component.
    Tier3,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CpuVendor {
    Intel,
    Amd,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    pub name: Field<String>,
    pub vendor: Field<CpuVendor>,
    pub cores: Field<u32>,
    pub threads: Field<u32>,
    pub performance_cores: Field<u32>,
    pub efficiency_cores: Field<u32>,
    pub max_clock_mhz: Field<u32>,
    /// Intel generation (e.g. 10 for i5-10400) or AMD Ryzen series (e.g. 5 for 5600X).
    pub generation: Field<u32>,
    pub perf_limit_pct: Field<f64>,
    pub throttling: Field<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryModuleInfo {
    pub capacity_gb: f64,
    pub locator: String,
    pub part_number: String,
    pub manufacturer: String,
    pub jedec_max_mts: Option<u32>,
    pub configured_mts: Option<u32>,
    pub rated_mts: Field<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Ddr3,
    Ddr4,
    Ddr5,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    pub total_gb: Field<f64>,
    pub kind: Field<MemoryKind>,
    pub modules: Vec<MemoryModuleInfo>,
    pub slots_total: Field<u32>,
    pub slots_used: Field<u32>,
    /// Slowest configured speed across modules (they all run at the same speed).
    pub configured_mts: Field<u32>,
    /// What the kit is built for (XMP/EXPO from part number, else SMBIOS max).
    pub rated_mts: Field<u32>,
    /// Max the CPU + chipset allow (e.g. Intel 10th gen on non-Z boards). None = no known cap.
    pub platform_cap_mts: Field<u32>,
    pub channels: Field<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuKind {
    Integrated,
    Dedicated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PcieLink {
    pub current_width: Option<u32>,
    pub max_width: Option<u32>,
    pub current_gen: Option<u32>,
    pub max_gen: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    pub vendor: GpuVendor,
    pub kind: Field<GpuKind>,
    pub vram_mb: Field<u32>,
    pub shared_mb: Field<u32>,
    pub driver_version: Field<String>,
    pub driver_date: Field<String>,
    pub driver_age_days: Field<u32>,
    pub pcie: Field<PcieLink>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Hdd,
    SataSsd,
    NvmeSsd,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriveHealth {
    Healthy,
    Warning,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub letter: String,
    pub size_gb: f64,
    pub free_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub model: String,
    pub media: Field<MediaType>,
    pub size_gb: f64,
    pub health: Field<DriveHealth>,
    pub volumes: Vec<VolumeInfo>,
    pub is_system: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    Ac,
    Battery,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerContext {
    pub is_laptop: Field<bool>,
    pub power_source: Field<PowerSource>,
    pub active_scheme_guid: Field<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowsInfo {
    pub name: Field<String>,
    pub build: Field<u32>,
    pub is_windows_11: Field<bool>,
    pub game_mode: Field<bool>,
    pub hags: Field<bool>,
    pub game_dvr: Field<bool>,
    pub windowed_optimizations: Field<bool>,
    pub vrr: Field<bool>,
    pub pending_reboot: Field<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoardVendor {
    Msi,
    Asus,
    Gigabyte,
    Asrock,
    Lenovo,
    Hp,
    Dell,
    Acer,
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareInfo {
    pub board_vendor: Field<BoardVendor>,
    pub board_manufacturer: Field<String>,
    pub board_product: Field<String>,
    /// Chipset parsed from the board name (e.g. "B460"), when present.
    pub chipset: Field<String>,
    pub bios_version: Field<String>,
    pub bios_date: Field<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HardwareProfile {
    pub schema_version: u32,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpus: Vec<GpuInfo>,
    pub storage: Vec<DriveInfo>,
    pub power: PowerContext,
    pub windows: WindowsInfo,
    pub firmware: FirmwareInfo,
    pub tier: Tier,
    /// Sources that failed; their fields are `unknown`.
    pub warnings: Vec<String>,
}

impl HardwareProfile {
    pub fn dedicated_gpu(&self) -> Option<&GpuInfo> {
        self.gpus
            .iter()
            .find(|g| g.kind.val() == Some(GpuKind::Dedicated))
    }
    pub fn integrated_gpu(&self) -> Option<&GpuInfo> {
        self.gpus
            .iter()
            .find(|g| g.kind.val() == Some(GpuKind::Integrated))
    }
    pub fn system_drive(&self) -> Option<&DriveInfo> {
        self.storage.iter().find(|d| d.is_system)
    }
    pub fn is_laptop(&self) -> bool {
        self.power.is_laptop.val() == Some(true)
    }
}
