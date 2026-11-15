//! Bottleneck classifier (goals.md §5.2).
//!
//! The session is cut into fixed windows (default 5 s). For each window:
//! 1. **Non-gameplay?** Loading (very long frames) or menu (steady frames, GPU almost idle)
//!    → excluded from metrics and classification.
//! 2. **Thermal / power**: on battery, or CPU `% Performance Limit` below threshold.
//! 3. **Memory**: RAM commit or VRAM above threshold *and* the window's stutters line up
//!    with hard-fault bursts. High memory use alone is not a bottleneck.
//! 4. **I/O**: stutters line up with disk-queue bursts.
//! 5. **GPU Busy ratio** = median(GPU busy / frametime): ≥ 0.90 → GPU-bound,
//!    ≤ 0.75 → CPU-bound (the GPU waits between frames), in between → mixed.
//!
//! Labels then go through **hysteresis**: the timeline only switches after N consecutive
//! windows agree, so a one-off blip never flips the verdict. The session label is the
//! majority of gameplay windows; if no label has a majority, it's honestly `mixed`.
//! All thresholds are configurable constants (`Thresholds`) with documented defaults.

use serde::{Deserialize, Serialize};

use super::metrics::{count_stutters, median};
use super::{Bottleneck, FrameSample, SystemSample, TimeRange};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thresholds {
    pub window_ms: f64,
    /// GPU busy / frametime at or above this → GPU-bound.
    pub gpu_bound_ratio: f64,
    /// At or below this → CPU-bound.
    pub cpu_bound_ratio: f64,
    pub memory_pct: f64,
    pub hard_faults_per_s: f64,
    pub disk_queue: f64,
    pub perf_limit_pct: f64,
    /// Share of a window's stutters that must coincide with a fault/disk burst.
    pub spike_correlation: f64,
    /// A stutter "coincides" with a system sample within this distance.
    pub correlation_ms: f64,
    /// Any frame this long marks a loading screen…
    pub loading_frame_ms: f64,
    /// …when the window's median frametime is also above this.
    pub loading_median_ms: f64,
    /// Menu: GPU busy ratio below this AND frametime coefficient of variation below `menu_cv`.
    pub menu_gpu_ratio: f64,
    pub menu_cv: f64,
    pub min_frames_per_window: usize,
    pub hysteresis_windows: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            window_ms: 5_000.0,
            gpu_bound_ratio: 0.90,
            cpu_bound_ratio: 0.75,
            memory_pct: 90.0,
            hard_faults_per_s: 150.0,
            disk_queue: 2.0,
            perf_limit_pct: 85.0,
            spike_correlation: 0.5,
            correlation_ms: 1_000.0,
            loading_frame_ms: 250.0,
            loading_median_ms: 50.0,
            menu_gpu_ratio: 0.20,
            menu_cv: 0.03,
            min_frames_per_window: 20,
            hysteresis_windows: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Gameplay,
    Loading,
    Menu,
    TooFewFrames,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowVerdict {
    pub range: TimeRange,
    pub kind: WindowKind,
    /// Raw label of this window (before hysteresis). `None` for non-gameplay windows.
    pub raw: Option<Bottleneck>,
    /// Label after hysteresis.
    pub smoothed: Option<Bottleneck>,
    pub gpu_ratio: Option<f64>,
    pub stutters: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    /// Stable id (i18n: `evidence.<id>`).
    pub id: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub label: Bottleneck,
    /// Share of gameplay windows agreeing with `label` (0–1).
    pub confidence: f64,
    pub evidence: Vec<Evidence>,
    pub windows: Vec<WindowVerdict>,
    pub non_gameplay: Vec<TimeRange>,
}

/// Loading screens at frame level: a cluster of ≥ `LOADING_CLUSTER` long frames within
/// `LOADING_CLUSTER_MS`, or one freeze ≥ `LOADING_FREEZE_MS`. An isolated long frame in the
/// middle of gameplay is a *stutter* and stays visible — never hidden as "loading".
pub const LOADING_CLUSTER: usize = 3;
pub const LOADING_CLUSTER_MS: f64 = 3_000.0;
pub const LOADING_FREEZE_MS: f64 = 2_000.0;

pub fn loading_spans(frames: &[FrameSample], th: &Thresholds) -> Vec<TimeRange> {
    let long: Vec<&FrameSample> = frames
        .iter()
        .filter(|f| f.frametime_ms >= th.loading_frame_ms)
        .collect();
    let mut spans: Vec<TimeRange> = Vec::new();
    for f in long.iter() {
        let cluster = long
            .iter()
            .filter(|g| (g.t_ms - f.t_ms).abs() <= LOADING_CLUSTER_MS)
            .count()
            >= LOADING_CLUSTER;
        if cluster || f.frametime_ms >= LOADING_FREEZE_MS {
            let r = TimeRange {
                start_ms: f.t_ms,
                end_ms: f.t_ms + f.frametime_ms,
            };
            match spans.last_mut() {
                // Merge spans separated by less than a second.
                Some(last) if r.start_ms - last.end_ms < 1_000.0 => {
                    last.end_ms = last.end_ms.max(r.end_ms)
                }
                _ => spans.push(r),
            }
        }
    }
    spans
}

fn window_ranges(frames: &[FrameSample], w: f64) -> Vec<TimeRange> {
    let (Some(first), Some(last)) = (frames.first(), frames.last()) else {
        return vec![];
    };
    let mut out = Vec::new();
    let mut s = first.t_ms;
    let end = last.t_ms + last.frametime_ms;
    while s < end {
        out.push(TimeRange {
            start_ms: s,
            end_ms: (s + w).min(end),
        });
        s += w;
    }
    out
}

fn sys_in<'a>(
    sys: &'a [SystemSample],
    r: &TimeRange,
) -> impl Iterator<Item = &'a SystemSample> + 'a {
    let r = *r;
    // A 1 Hz sample describes the second before it: include samples up to one second past the window.
    sys.iter()
        .filter(move |s| s.t_ms >= r.start_ms && s.t_ms < r.end_ms + 1_000.0)
}

/// Share of stutter timestamps that have a system sample within `ms` where `pred` holds.
fn correlated(
    stutter_t: &[f64],
    sys: &[SystemSample],
    ms: f64,
    pred: impl Fn(&SystemSample) -> bool,
) -> f64 {
    if stutter_t.is_empty() {
        return 0.0;
    }
    let hits = stutter_t
        .iter()
        .filter(|&&t| sys.iter().any(|s| (s.t_ms - t).abs() <= ms && pred(s)))
        .count();
    hits as f64 / stutter_t.len() as f64
}

/// `frames` are the window's frames with loading-span frames already removed;
/// `loading_ms` is how much of the window those removed frames covered.
fn classify_window(
    frames: &[&FrameSample],
    loading_ms: f64,
    sys: &[SystemSample],
    r: &TimeRange,
    th: &Thresholds,
) -> (WindowKind, Option<Bottleneck>, Option<f64>, u32) {
    let window_ms = r.end_ms - r.start_ms;
    if loading_ms >= window_ms * 0.5 {
        return (WindowKind::Loading, None, None, 0);
    }
    if frames.len() < th.min_frames_per_window {
        return (WindowKind::TooFewFrames, None, None, 0);
    }
    let ft: Vec<f64> = frames.iter().map(|f| f.frametime_ms).collect();
    let med = median(&ft);
    let ratios: Vec<f64> = frames
        .iter()
        .filter_map(|f| f.gpu_busy_ms.map(|g| (g / f.frametime_ms).min(1.0)))
        .collect();
    let gpu_ratio = (ratios.len() * 2 >= frames.len()).then(|| median(&ratios));

    if med >= th.loading_median_ms && ft.iter().any(|&v| v >= th.loading_frame_ms) {
        return (WindowKind::Loading, None, gpu_ratio, 0);
    }
    let mean = ft.iter().sum::<f64>() / ft.len() as f64;
    let cv = (ft.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / ft.len() as f64).sqrt() / mean;
    if gpu_ratio.is_some_and(|g| g < th.menu_gpu_ratio) && cv < th.menu_cv {
        return (WindowKind::Menu, None, gpu_ratio, 0);
    }

    let (n_stutter, idx) = count_stutters(&ft);
    let stutter_t: Vec<f64> = idx.iter().map(|&i| frames[i].t_ms).collect();
    let ws: Vec<SystemSample> = sys_in(sys, r).copied().collect();

    // Thermal / power
    let battery = ws.iter().any(|s| s.on_battery == Some(true));
    let limits: Vec<f64> = ws.iter().filter_map(|s| s.perf_limit_pct).collect();
    if battery || (!limits.is_empty() && median(&limits) < th.perf_limit_pct) {
        return (
            WindowKind::Gameplay,
            Some(Bottleneck::ThermalOrPowerLimited),
            gpu_ratio,
            n_stutter,
        );
    }
    // Memory: pressure + stutters correlated with hard-fault bursts
    let pressure = ws.iter().any(|s| {
        s.ram_commit_pct.is_some_and(|v| v >= th.memory_pct)
            || s.vram_usage_pct.is_some_and(|v| v >= th.memory_pct)
    });
    if pressure
        && n_stutter >= 2
        && correlated(&stutter_t, sys, th.correlation_ms, |s| {
            s.hard_faults_per_s
                .is_some_and(|v| v >= th.hard_faults_per_s)
        }) >= th.spike_correlation
    {
        return (
            WindowKind::Gameplay,
            Some(Bottleneck::MemoryBound),
            gpu_ratio,
            n_stutter,
        );
    }
    // I/O: stutters correlated with disk queue bursts
    if n_stutter >= 2
        && correlated(&stutter_t, sys, th.correlation_ms, |s| {
            s.disk_queue.is_some_and(|v| v >= th.disk_queue)
        }) >= th.spike_correlation
    {
        return (
            WindowKind::Gameplay,
            Some(Bottleneck::IoBound),
            gpu_ratio,
            n_stutter,
        );
    }
    let label = match gpu_ratio {
        Some(g) if g >= th.gpu_bound_ratio => Bottleneck::GpuBound,
        Some(g) if g <= th.cpu_bound_ratio => Bottleneck::CpuBound,
        Some(_) => Bottleneck::Mixed,
        None => Bottleneck::Unknown,
    };
    (WindowKind::Gameplay, Some(label), gpu_ratio, n_stutter)
}

pub fn classify(frames: &[FrameSample], sys: &[SystemSample], th: &Thresholds) -> Classification {
    let loading = loading_spans(frames, th);
    let in_loading = |f: &FrameSample| loading.iter().any(|r| r.contains(f.t_ms));
    let mut windows: Vec<WindowVerdict> = window_ranges(frames, th.window_ms)
        .into_iter()
        .map(|r| {
            let (load, play): (Vec<&FrameSample>, Vec<&FrameSample>) = frames
                .iter()
                .filter(|f| r.contains(f.t_ms))
                .partition(|f| in_loading(f));
            let loading_ms: f64 = load
                .iter()
                .map(|f| f.frametime_ms.min(r.end_ms - f.t_ms))
                .sum();
            let (kind, raw, gpu_ratio, stutters) = classify_window(&play, loading_ms, sys, &r, th);
            WindowVerdict {
                range: r,
                kind,
                raw,
                smoothed: None,
                gpu_ratio,
                stutters,
            }
        })
        .collect();

    // Hysteresis over gameplay windows. The initial label is the mode of the first
    // `hysteresis_windows` windows, so a transitional first window can't seed the timeline.
    let first: Vec<Bottleneck> = windows
        .iter()
        .filter(|w| w.kind == WindowKind::Gameplay)
        .take(th.hysteresis_windows.max(1))
        .map(|w| w.raw.unwrap_or(Bottleneck::Unknown))
        .collect();
    let seed = first
        .iter()
        .copied()
        .max_by_key(|b| first.iter().filter(|x| *x == b).count());
    let mut current: Option<Bottleneck> = seed;
    let mut pending: Option<(Bottleneck, usize)> = None;
    for w in windows
        .iter_mut()
        .filter(|w| w.kind == WindowKind::Gameplay)
    {
        let raw = w.raw.unwrap_or(Bottleneck::Unknown);
        match current {
            None => current = Some(raw),
            Some(c) if c == raw => pending = None,
            Some(_) => {
                let n = match pending {
                    Some((p, n)) if p == raw => n + 1,
                    _ => 1,
                };
                if n >= th.hysteresis_windows {
                    current = Some(raw);
                    pending = None;
                } else {
                    pending = Some((raw, n));
                }
            }
        }
        w.smoothed = current;
    }

    let gameplay: Vec<&WindowVerdict> = windows
        .iter()
        .filter(|w| w.kind == WindowKind::Gameplay)
        .collect();
    // Frame-level loading spans + whole menu/loading windows, merged.
    let mut non_gameplay: Vec<TimeRange> = loading.clone();
    non_gameplay.extend(
        windows
            .iter()
            .filter(|w| matches!(w.kind, WindowKind::Loading | WindowKind::Menu))
            .map(|w| w.range),
    );
    non_gameplay.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
    let mut merged: Vec<TimeRange> = Vec::new();
    for r in non_gameplay {
        match merged.last_mut() {
            Some(last) if r.start_ms <= last.end_ms => last.end_ms = last.end_ms.max(r.end_ms),
            _ => merged.push(r),
        }
    }
    let non_gameplay = merged;

    let (label, confidence) = if gameplay.is_empty() {
        (Bottleneck::Unknown, 0.0)
    } else {
        let mut counts: Vec<(Bottleneck, usize)> = Vec::new();
        for w in &gameplay {
            let l = w.smoothed.unwrap_or(Bottleneck::Unknown);
            match counts.iter_mut().find(|(b, _)| *b == l) {
                Some((_, n)) => *n += 1,
                None => counts.push((l, 1)),
            }
        }
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        let (top, n) = counts[0];
        let share = n as f64 / gameplay.len() as f64;
        if share >= 0.5 {
            (top, share)
        } else {
            (Bottleneck::Mixed, share)
        }
    };

    let ratios: Vec<f64> = gameplay.iter().filter_map(|w| w.gpu_ratio).collect();
    let mut evidence = vec![
        Evidence {
            id: "gameplay_windows".into(),
            value: gameplay.len() as f64,
        },
        Evidence {
            id: "excluded_windows".into(),
            value: non_gameplay.len() as f64,
        },
        Evidence {
            id: "stutters".into(),
            value: gameplay.iter().map(|w| w.stutters as f64).sum(),
        },
    ];
    if !ratios.is_empty() {
        evidence.push(Evidence {
            id: "gpu_busy_ratio".into(),
            value: (median(&ratios) * 100.0).round() / 100.0,
        });
    }
    let limits: Vec<f64> = sys.iter().filter_map(|s| s.perf_limit_pct).collect();
    if !limits.is_empty() {
        evidence.push(Evidence {
            id: "perf_limit_min".into(),
            value: limits.iter().copied().fold(f64::INFINITY, f64::min),
        });
    }
    let mem: Vec<f64> = sys
        .iter()
        .flat_map(|s| [s.ram_commit_pct, s.vram_usage_pct])
        .flatten()
        .collect();
    if !mem.is_empty() {
        evidence.push(Evidence {
            id: "memory_peak_pct".into(),
            value: mem.iter().copied().fold(0.0, f64::max),
        });
    }

    Classification {
        label,
        confidence: (confidence * 100.0).round() / 100.0,
        evidence,
        windows,
        non_gameplay,
    }
}

/// Convenience: ranges to exclude from metrics (loading screens and menus).
pub fn non_gameplay_ranges(
    frames: &[FrameSample],
    sys: &[SystemSample],
    th: &Thresholds,
) -> Vec<TimeRange> {
    classify(frames, sys, th).non_gameplay
}
