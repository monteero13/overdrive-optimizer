//! Module B — telemetry & bottleneck classification (goals.md §5.2).
//!
//! - [`trace`]: PresentMon CSV parsing (1.x and 2.x columns), streaming line by line.
//! - [`metrics`]: avg FPS, 1 % / 0.1 % lows, percentiles, stdev, stutters.
//! - [`classify`]: windowed bottleneck classifier with hysteresis and loading/menu exclusion.
//!
//! The primary signal is **GPU Busy vs frametime** (PresentMon 2.x), not utilisation %.

pub mod classify;
pub mod metrics;
pub mod trace;

use serde::{Deserialize, Serialize};

/// One presented frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameSample {
    /// Start of the frame, ms since capture start.
    pub t_ms: f64,
    /// Time between this present and the previous one (= 1000 / instantaneous FPS).
    pub frametime_ms: f64,
    /// Time the GPU was actually busy on this frame (PresentMon 2.x `GPUBusy`, 1.x `MsGPUActive`).
    pub gpu_busy_ms: Option<f64>,
    pub display_latency_ms: Option<f64>,
}

/// System counters sampled ~1 Hz during a session.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSample {
    pub t_ms: f64,
    pub ram_commit_pct: Option<f64>,
    pub vram_usage_pct: Option<f64>,
    /// Hard page faults that had to read from disk (PDH `\Memory\Page Reads/sec`).
    pub hard_faults_per_s: Option<f64>,
    pub disk_queue: Option<f64>,
    pub perf_limit_pct: Option<f64>,
    pub on_battery: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_ms: f64,
    pub end_ms: f64,
}

impl TimeRange {
    pub fn contains(&self, t: f64) -> bool {
        t >= self.start_ms && t < self.end_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bottleneck {
    GpuBound,
    CpuBound,
    MemoryBound,
    IoBound,
    ThermalOrPowerLimited,
    Mixed,
    Unknown,
}

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("telemetry tool not found: {0}")]
    ToolMissing(String),
    #[error("could not start capture: {0}")]
    Start(String),
    #[error("capture not running")]
    NotRunning,
    #[error("trace parse error: {0}")]
    Parse(String),
}

/// Frame source for a running game (PresentMon in `overdrive-win`, fixtures in tests).
pub trait TelemetrySource {
    fn start(&mut self, pid: u32) -> Result<(), TelemetryError>;
    /// Non-blocking: frames captured since the last call.
    fn poll(&mut self) -> Vec<FrameSample>;
    fn stop(&mut self) -> Result<(), TelemetryError>;
}
