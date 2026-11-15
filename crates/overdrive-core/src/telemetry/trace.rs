//! PresentMon CSV parsing. Header-driven, so it accepts both column sets:
//!
//! | Meaning        | PresentMon 2.x  | PresentMon 1.x          |
//! |----------------|-----------------|-------------------------|
//! | time (ms)      | `CPUStartTime`  | `TimeInSeconds` (× 1000)|
//! | frametime (ms) | `FrameTime`     | `MsBetweenPresents`     |
//! | GPU busy (ms)  | `GPUBusy`       | `MsGPUActive`           |
//! | display latency| `DisplayLatency`| `MsUntilDisplayed`      |
//!
//! Values PresentMon writes as `NA` become `None`. If there is no time column, time is the
//! running sum of frametimes. Also parses Overdrive's own system-sample CSV.

use super::{FrameSample, SystemSample, TelemetryError};

#[derive(Debug, Clone, Copy)]
struct Columns {
    time: Option<(usize, f64)>, // (index, multiplier to ms)
    frametime: usize,
    gpu_busy: Option<usize>,
    display_latency: Option<usize>,
    frame_type: Option<usize>,
}

/// Incremental parser: feed it stdout lines as PresentMon prints them.
#[derive(Debug, Default)]
pub struct PresentMonParser {
    cols: Option<Columns>,
    clock_ms: f64,
    pub rejected: u64,
}

fn num(s: Option<&str>) -> Option<f64> {
    let s = s?.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("NA") {
        return None;
    }
    s.parse().ok().filter(|v: &f64| v.is_finite())
}

impl PresentMonParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_header(&self) -> bool {
        self.cols.is_some()
    }

    /// Returns a frame for data lines, `None` for the header, blanks and junk.
    pub fn push_line(&mut self, line: &str) -> Option<FrameSample> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            return None;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if self.cols.is_none()
            || fields
                .iter()
                .any(|f| f.trim() == "FrameTime" || f.trim() == "MsBetweenPresents")
        {
            let find = |name: &str| {
                fields
                    .iter()
                    .position(|f| f.trim().eq_ignore_ascii_case(name))
            };
            let frametime = find("FrameTime").or_else(|| find("MsBetweenPresents"));
            if let Some(frametime) = frametime {
                self.cols = Some(Columns {
                    time: find("CPUStartTime")
                        .map(|i| (i, 1.0))
                        .or_else(|| find("TimeInSeconds").map(|i| (i, 1000.0))),
                    frametime,
                    gpu_busy: find("GPUBusy").or_else(|| find("MsGPUActive")),
                    display_latency: find("DisplayLatency").or_else(|| find("MsUntilDisplayed")),
                    frame_type: find("FrameType"),
                });
                self.clock_ms = 0.0;
                return None;
            }
            if self.cols.is_none() {
                self.rejected += 1;
                return None;
            }
        }
        let c = self.cols?;
        // PresentMon 2.x frame generation: only count application-rendered frames.
        if let Some(i) = c.frame_type {
            if let Some(ft) = fields.get(i).map(|s| s.trim()) {
                if !ft.is_empty()
                    && !ft.eq_ignore_ascii_case("Application")
                    && !ft.eq_ignore_ascii_case("NA")
                {
                    return None;
                }
            }
        }
        let Some(frametime) = num(fields.get(c.frametime).copied()).filter(|&v| v > 0.0) else {
            self.rejected += 1;
            return None;
        };
        let t_ms = match c
            .time
            .and_then(|(i, k)| num(fields.get(i).copied()).map(|v| v * k))
        {
            Some(t) => t,
            None => {
                self.clock_ms += frametime;
                self.clock_ms
            }
        };
        Some(FrameSample {
            t_ms,
            frametime_ms: frametime,
            gpu_busy_ms: c.gpu_busy.and_then(|i| num(fields.get(i).copied())),
            display_latency_ms: c.display_latency.and_then(|i| num(fields.get(i).copied())),
        })
    }
}

/// Parse a whole PresentMon CSV file.
pub fn parse_presentmon_csv(text: &str) -> Result<Vec<FrameSample>, TelemetryError> {
    let mut p = PresentMonParser::new();
    let frames: Vec<FrameSample> = text.lines().filter_map(|l| p.push_line(l)).collect();
    if !p.has_header() {
        return Err(TelemetryError::Parse(
            "no PresentMon header (FrameTime / MsBetweenPresents)".into(),
        ));
    }
    Ok(frames)
}

pub const SYSTEM_CSV_HEADER: &str =
    "t_ms,ram_commit_pct,vram_usage_pct,hard_faults_per_s,disk_queue,perf_limit_pct,on_battery";

pub fn parse_system_csv(text: &str) -> Result<Vec<SystemSample>, TelemetryError> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| TelemetryError::Parse("empty system csv".into()))?;
    if header.trim() != SYSTEM_CSV_HEADER {
        return Err(TelemetryError::Parse(format!(
            "unexpected system csv header: {header}"
        )));
    }
    lines
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            Ok(SystemSample {
                t_ms: num(f.first().copied())
                    .ok_or_else(|| TelemetryError::Parse(format!("bad t_ms: {l}")))?,
                ram_commit_pct: num(f.get(1).copied()),
                vram_usage_pct: num(f.get(2).copied()),
                hard_faults_per_s: num(f.get(3).copied()),
                disk_queue: num(f.get(4).copied()),
                perf_limit_pct: num(f.get(5).copied()),
                on_battery: f.get(6).and_then(|s| match s.trim() {
                    "1" | "true" => Some(true),
                    "0" | "false" => Some(false),
                    _ => None,
                }),
            })
        })
        .collect()
}

fn opt(v: Option<f64>) -> String {
    v.map_or("NA".into(), |x| format!("{x:.3}"))
}

/// Writes frames in PresentMon 2.x column layout (subset of columns, same names).
pub fn to_presentmon_v2_csv(app: &str, pid: u32, frames: &[FrameSample]) -> String {
    let mut s = String::from("Application,ProcessID,SwapChainAddress,PresentRuntime,SyncInterval,PresentFlags,AllowsTearing,PresentMode,FrameType,CPUStartTime,FrameTime,CPUBusy,CPUWait,GPULatency,GPUTime,GPUBusy,GPUWait,DisplayLatency,DisplayedTime\n");
    for f in frames {
        let gpu = f.gpu_busy_ms;
        let cpu_busy = gpu.map(|g| (f.frametime_ms - g).max(0.0) * 0.6 + f.frametime_ms * 0.3);
        s.push_str(&format!(
            "{app},{pid},0x000001A2B3C4D5E0,DXGI,0,0,1,Hardware: Independent Flip,Application,{:.4},{:.4},{},{},{},{},{},{},{},{:.4}\n",
            f.t_ms,
            f.frametime_ms,
            opt(cpu_busy),
            opt(cpu_busy.map(|c| (f.frametime_ms - c).max(0.0))),
            opt(gpu.map(|g| g * 0.2)),
            opt(gpu.map(|g| g * 1.05)),
            opt(gpu),
            opt(gpu.map(|g| (f.frametime_ms - g).max(0.0))),
            opt(f.display_latency_ms),
            f.frametime_ms,
        ));
    }
    s
}

/// Writes frames in PresentMon 1.x column layout.
pub fn to_presentmon_v1_csv(app: &str, pid: u32, frames: &[FrameSample]) -> String {
    let mut s = String::from("Application,ProcessID,SwapChainAddress,Runtime,SyncInterval,PresentFlags,AllowsTearing,PresentMode,WasBatched,DwmNotified,Dropped,TimeInSeconds,MsInPresentAPI,MsBetweenPresents,MsBetweenDisplayChange,MsUntilRenderComplete,MsUntilDisplayed,MsGPUActive,MsGPUVideoActive\n");
    for f in frames {
        s.push_str(&format!(
            "{app},{pid},0x000001A2B3C4D5E0,DXGI,0,0,1,Hardware: Independent Flip,0,0,0,{:.6},0.150,{:.4},{:.4},{},{},{},0.000\n",
            f.t_ms / 1000.0,
            f.frametime_ms,
            f.frametime_ms,
            opt(f.gpu_busy_ms.map(|g| g * 1.1)),
            opt(f.display_latency_ms),
            opt(f.gpu_busy_ms),
        ));
    }
    s
}

pub fn to_system_csv(samples: &[SystemSample]) -> String {
    let mut s = format!("{SYSTEM_CSV_HEADER}\n");
    for x in samples {
        s.push_str(&format!(
            "{:.0},{},{},{},{},{},{}\n",
            x.t_ms,
            opt(x.ram_commit_pct),
            opt(x.vram_usage_pct),
            opt(x.hard_faults_per_s),
            opt(x.disk_queue),
            opt(x.perf_limit_pct),
            x.on_battery.map_or("NA", |b| if b { "1" } else { "0" }),
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_and_v1_parse_identically() {
        let frames = vec![
            FrameSample {
                t_ms: 0.0,
                frametime_ms: 16.6,
                gpu_busy_ms: Some(15.9),
                display_latency_ms: Some(20.1),
            },
            FrameSample {
                t_ms: 16.6,
                frametime_ms: 17.0,
                gpu_busy_ms: None,
                display_latency_ms: None,
            },
        ];
        let a = parse_presentmon_csv(&to_presentmon_v2_csv("game.exe", 42, &frames)).unwrap();
        let b = parse_presentmon_csv(&to_presentmon_v1_csv("game.exe", 42, &frames)).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 2);
        for (x, y) in a.iter().zip(&b) {
            assert!(
                (x.t_ms - y.t_ms).abs() < 0.01 && (x.frametime_ms - y.frametime_ms).abs() < 0.01
            );
            assert_eq!(x.gpu_busy_ms.is_some(), y.gpu_busy_ms.is_some());
        }
        assert_eq!(a[1].gpu_busy_ms, None, "NA → None");
    }

    #[test]
    fn streaming_skips_junk_and_generated_frames() {
        let mut p = PresentMonParser::new();
        assert!(p.push_line("Started recording...").is_none());
        assert!(p
            .push_line("Application,ProcessID,FrameType,CPUStartTime,FrameTime,GPUBusy")
            .is_none());
        assert!(p.push_line("g.exe,1,Application,0.0,16.0,15.0").is_some());
        assert!(
            p.push_line("g.exe,1,AMD_AFMF,8.0,8.0,1.0").is_none(),
            "frame-generation frames excluded"
        );
        assert!(
            p.push_line("g.exe,1,Application,16.0,NA,15.0").is_none(),
            "no frametime"
        );
        assert!(p.push_line("").is_none());
        assert_eq!(p.rejected, 2);
    }

    #[test]
    fn no_time_column_accumulates() {
        let csv = "MsBetweenPresents,MsGPUActive\n10,9\n20,9\n";
        let f = parse_presentmon_csv(csv).unwrap();
        assert_eq!((f[0].t_ms, f[1].t_ms), (10.0, 30.0));
        assert!(parse_presentmon_csv("garbage\n1,2,3").is_err());
    }

    #[test]
    fn system_csv_roundtrip() {
        let s = vec![SystemSample {
            t_ms: 1000.0,
            ram_commit_pct: Some(91.5),
            on_battery: Some(true),
            ..Default::default()
        }];
        let back = parse_system_csv(&to_system_csv(&s)).unwrap();
        assert_eq!(back[0].ram_commit_pct, Some(91.5));
        assert_eq!(back[0].on_battery, Some(true));
        assert_eq!(back[0].disk_queue, None);
    }
}
