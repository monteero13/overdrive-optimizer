//! Session metrics. Definitions (documented so results are comparable and honest):
//!
//! - **avg FPS** = frames / gameplay time (not the mean of per-frame FPS, which overweights fast frames).
//! - **1 % low / 0.1 % low** = 1000 / P99 (P99.9) frametime — the percentile definition
//!   (PresentMon/CapFrameX "P1"/"P0.1"). Stable with few frames, easy to explain.
//! - **stdev** of frametime in ms.
//! - **stutter** = a frame > 2× the rolling median of the previous 30 frames AND at least
//!   8 ms slower than it (so 4 → 9 ms at 250 FPS, which nobody notices, doesn't count).
//! - Frames inside `exclude` ranges (loading screens, menus) are ignored.

use serde::{Deserialize, Serialize};

use super::{FrameSample, TimeRange};

pub const STUTTER_FACTOR: f64 = 2.0;
pub const STUTTER_MIN_EXTRA_MS: f64 = 8.0;
pub const STUTTER_WINDOW: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetrics {
    pub frames: usize,
    pub gameplay_secs: f64,
    pub avg_fps: f64,
    pub low_1: f64,
    pub low_01: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub stdev_ms: f64,
    pub stutters: u32,
}

/// Percentile with linear interpolation on a sorted slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
}

pub fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    percentile(&s, 50.0)
}

pub fn count_stutters(frametimes: &[f64]) -> (u32, Vec<usize>) {
    let mut idx = Vec::new();
    for i in STUTTER_WINDOW..frametimes.len() {
        let m = median(&frametimes[i - STUTTER_WINDOW..i]);
        let ft = frametimes[i];
        if ft > m * STUTTER_FACTOR && ft > m + STUTTER_MIN_EXTRA_MS {
            idx.push(i);
        }
    }
    (idx.len() as u32, idx)
}

pub fn compute(frames: &[FrameSample], exclude: &[TimeRange]) -> SessionMetrics {
    let ft: Vec<f64> = frames
        .iter()
        .filter(|f| !exclude.iter().any(|r| r.contains(f.t_ms)))
        .map(|f| f.frametime_ms)
        .collect();
    if ft.is_empty() {
        return SessionMetrics {
            frames: 0,
            gameplay_secs: 0.0,
            avg_fps: 0.0,
            low_1: 0.0,
            low_01: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            stdev_ms: 0.0,
            stutters: 0,
        };
    }
    let total_ms: f64 = ft.iter().sum();
    let mean = total_ms / ft.len() as f64;
    let var = ft.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / ft.len() as f64;
    let mut sorted = ft.clone();
    sorted.sort_by(f64::total_cmp);
    let p99 = percentile(&sorted, 99.0);
    let p999 = percentile(&sorted, 99.9);
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    SessionMetrics {
        frames: ft.len(),
        gameplay_secs: r1(total_ms / 1000.0),
        avg_fps: r1(ft.len() as f64 / (total_ms / 1000.0)),
        low_1: r1(1000.0 / p99),
        low_01: r1(1000.0 / p999),
        p50_ms: r2(percentile(&sorted, 50.0)),
        p95_ms: r2(percentile(&sorted, 95.0)),
        p99_ms: r2(p99),
        stdev_ms: r2(var.sqrt()),
        stutters: count_stutters(&ft).0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(ft: &[f64]) -> Vec<FrameSample> {
        let mut t = 0.0;
        ft.iter()
            .map(|&f| {
                let s = FrameSample {
                    t_ms: t,
                    frametime_ms: f,
                    gpu_busy_ms: None,
                    display_latency_ms: None,
                };
                t += f;
                s
            })
            .collect()
    }

    #[test]
    fn steady_60fps() {
        let m = compute(&frames(&[16.667; 600]), &[]);
        assert_eq!(m.avg_fps, 60.0);
        assert_eq!(m.low_1, 60.0);
        assert_eq!(m.stutters, 0);
        assert_eq!(m.stdev_ms, 0.0);
        assert_eq!(m.gameplay_secs, 10.0);
    }

    #[test]
    fn spikes_hit_lows_and_stutters_not_average_much() {
        let mut ft = vec![10.0; 1000];
        for i in (100..1000).step_by(100) {
            ft[i] = 50.0; // 9 spikes, 0.9 % of frames
        }
        let m = compute(&frames(&ft), &[]);
        assert!(m.avg_fps > 95.0, "{}", m.avg_fps);
        assert!(m.low_01 < 25.0, "0.1% low reflects spikes: {}", m.low_01);
        assert_eq!(m.stutters, 9);
    }

    #[test]
    fn small_spikes_at_high_fps_are_not_stutters() {
        let mut ft = vec![4.0; 500];
        ft[200] = 9.0; // 2.25× but only +5 ms
        assert_eq!(compute(&frames(&ft), &[]).stutters, 0);
    }

    #[test]
    fn excluded_ranges_are_ignored() {
        let mut ft = vec![300.0; 10]; // loading screen (3 s)
        ft.extend(vec![10.0; 500]);
        let all = frames(&ft);
        let m = compute(
            &all,
            &[TimeRange {
                start_ms: 0.0,
                end_ms: 3000.0,
            }],
        );
        assert_eq!(m.frames, 500);
        assert_eq!(m.avg_fps, 100.0);
    }

    #[test]
    fn empty_is_zero_not_nan() {
        let m = compute(&[], &[]);
        assert_eq!(m.frames, 0);
        assert!(!m.avg_fps.is_nan());
    }
}
