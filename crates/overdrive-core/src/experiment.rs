//! A/B verification protocol (goals.md §5.6) — the core differentiator.
//!
//! **Staged introduction**: measure a baseline, then add ONE tweak at a time and compare the
//! new configuration against the previous one. A tweak is:
//! - **kept** if the gain's 95 % confidence interval is entirely above zero and the gain is
//!   at least `min_gain_pct` (practically noticeable, not just statistically);
//! - **harmful** if the interval is entirely below zero by at least the same margin → blacklisted
//!   for this game on this machine;
//! - **no measurable benefit** if the interval sits inside ±`equivalence_pct`, or if after
//!   `max_runs` sessions it still can't be told apart from noise → never applied again;
//! - otherwise **inconclusive** → one more session.
//!
//! The protocol is *stateless*: everything is derived from the journal's measured sessions
//! and stored verdicts, so it survives restarts, crashes and updates. Sessions whose context
//! (driver, game settings, power source, hardware) differs from the latest one are not
//! compared — a driver update silently restarts the measurement instead of corrupting it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::catalog::Measure;
use crate::telemetry::metrics::SessionMetrics;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentSettings {
    /// Sessions needed per configuration before comparing.
    pub min_runs: usize,
    /// After this many sessions of the candidate, "still can't tell" means "no measurable benefit".
    pub max_runs: usize,
    /// Smallest gain (%) worth keeping a change for.
    pub min_gain_pct: f64,
    /// A change whose effect's CI lies within ±this (%) does nothing measurable.
    pub equivalence_pct: f64,
    /// Sessions shorter than this (gameplay, excluding loading/menus) don't count.
    pub min_gameplay_secs: f64,
}

impl Default for ExperimentSettings {
    /// Calibrated with `verdicts_are_robust_across_noise_seeds` (tests/session.rs):
    /// ~2 % session-to-session noise, effects of 5–30 %, two null and one harmful tweak.
    fn default() -> Self {
        Self {
            min_runs: 3,
            max_runs: 8,
            min_gain_pct: 3.0,
            equivalence_pct: 3.0,
            min_gameplay_secs: 60.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Kept,
    NoBenefit,
    Harmful,
    Inconclusive,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Kept => "kept",
            Verdict::NoBenefit => "no_benefit",
            Verdict::Harmful => "harmful",
            Verdict::Inconclusive => "inconclusive",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "kept" => Verdict::Kept,
            "no_benefit" => Verdict::NoBenefit,
            "harmful" => Verdict::Harmful,
            "inconclusive" => Verdict::Inconclusive,
            _ => return None,
        })
    }
    pub fn is_final(self) -> bool {
        self != Verdict::Inconclusive
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub verdict: Verdict,
    /// Relative improvement in % (positive = better, whatever the metric's direction).
    pub delta_pct: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub runs_base: usize,
    pub runs_candidate: usize,
    /// True when NoBenefit was reached by exhausting `max_runs`, not by equivalence.
    pub exhausted: bool,
}

/// Two-sided 99 % Student-t critical value. Decisions use 99 % (not 95 %) because the
/// data is looked at after every session: repeated looks inflate false positives.
pub fn t99(df: f64) -> f64 {
    const T: [f64; 30] = [
        63.657, 9.925, 5.841, 4.604, 4.032, 3.707, 3.499, 3.355, 3.250, 3.169, 3.106, 3.055, 3.012,
        2.977, 2.947, 2.921, 2.898, 2.878, 2.861, 2.845, 2.831, 2.819, 2.807, 2.797, 2.787, 2.779,
        2.771, 2.763, 2.756, 2.750,
    ];
    if df < 1.0 {
        return T[0];
    }
    let i = df.floor() as usize;
    if i >= 30 {
        2.576
    } else {
        T[i - 1]
    }
}

/// Two-sided 95 % Student-t critical value.
pub fn t95(df: f64) -> f64 {
    const T: [f64; 30] = [
        12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160,
        2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056,
        2.052, 2.048, 2.045, 2.042,
    ];
    if df < 1.0 {
        return T[0];
    }
    let i = df.floor() as usize;
    if i >= 30 {
        1.96
    } else {
        T[i - 1]
    }
}

fn mean_var(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    let var = if v.len() > 1 {
        v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0)
    } else {
        0.0
    };
    (m, var)
}

/// Welch comparison of a metric between two configurations.
/// `higher_is_better`: true for the 1 % low, false for stutters/minute.
pub fn compare(
    base: &[f64],
    cand: &[f64],
    higher_is_better: bool,
    s: &ExperimentSettings,
) -> Comparison {
    let (mb, vb) = mean_var(base);
    let (mc, vc) = mean_var(cand);
    let (nb, nc) = (base.len() as f64, cand.len() as f64);
    let mk = |verdict, d, lo, hi, exhausted| Comparison {
        verdict,
        delta_pct: d,
        ci_low: lo,
        ci_high: hi,
        runs_base: base.len(),
        runs_candidate: cand.len(),
        exhausted,
    };
    if base.is_empty() || cand.is_empty() {
        return mk(
            Verdict::Inconclusive,
            0.0,
            f64::NEG_INFINITY,
            f64::INFINITY,
            false,
        );
    }
    if !higher_is_better && mb < 0.1 && mc < 0.1 {
        // Practically no stutters either way: nothing to improve.
        return mk(Verdict::NoBenefit, 0.0, 0.0, 0.0, false);
    }
    let denom = mb.abs().max(1e-9);
    let delta = if higher_is_better {
        (mc - mb) / denom * 100.0
    } else {
        (mb - mc) / denom * 100.0
    };
    let se2 = vb / nb + vc / nc;
    let se = se2.sqrt() / denom * 100.0;
    // Welch–Satterthwaite degrees of freedom.
    let df = if se2 > 0.0 {
        se2.powi(2)
            / ((vb / nb).powi(2) / (nb - 1.0).max(1.0) + (vc / nc).powi(2) / (nc - 1.0).max(1.0))
    } else {
        nb + nc - 2.0
    };
    let half = t99(df.max(1.0)) * se;
    let (lo, hi) = (delta - half, delta + half);
    let r = |v: f64| (v * 10.0).round() / 10.0;
    let verdict = if lo > 0.0 && delta >= s.min_gain_pct {
        Verdict::Kept
    } else if hi < 0.0 && delta <= -s.min_gain_pct {
        Verdict::Harmful
    } else if lo >= -s.equivalence_pct && hi <= s.equivalence_pct {
        Verdict::NoBenefit
    } else {
        Verdict::Inconclusive
    };
    mk(verdict, r(delta), r(lo), r(hi), false)
}

pub fn metric_value(m: &SessionMetrics, measure: Measure) -> f64 {
    match measure {
        Measure::Frametime | Measure::None => m.low_1,
        // A frame cap trades average FPS for evenness: judge it by frametime spread.
        Measure::Pacing => m.stdev_ms,
        Measure::BackgroundNoise => {
            if m.gameplay_secs > 0.0 {
                m.stutters as f64 / (m.gameplay_secs / 60.0)
            } else {
                0.0
            }
        }
    }
}

/// A measured session as the protocol sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPoint {
    /// Measured tweaks active during the session (sorted).
    pub tweak_set: Vec<String>,
    pub context: serde_json::Value,
    pub metrics: SessionMetrics,
    pub valid: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Play a session with exactly these measured tweaks active.
    Run {
        tweaks: Vec<String>,
        testing: Option<String>,
    },
    /// Enough data: record this verdict, then ask again.
    Decide {
        tweak: String,
        comparison: Comparison,
    },
    /// Every candidate has a verdict.
    Converged { kept: Vec<String> },
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut v = v.to_vec();
    v.sort();
    v
}

/// What to do next for one game. `candidates` are in experiment order.
pub fn next(
    candidates: &[String],
    decided: &HashMap<String, Verdict>,
    sessions: &[SessionPoint],
    measure_of: &dyn Fn(&str) -> Measure,
    s: &ExperimentSettings,
) -> Next {
    let kept: Vec<String> = candidates
        .iter()
        .filter(|c| decided.get(*c) == Some(&Verdict::Kept))
        .cloned()
        .collect();
    let current = candidates
        .iter()
        .find(|c| !decided.get(*c).is_some_and(|v| v.is_final()));

    // Only sessions comparable with the latest one count.
    let latest_ctx = sessions
        .iter()
        .rev()
        .find(|p| p.valid)
        .map(|p| p.context.clone());
    let runs_of = |set: &[String]| -> Vec<&SessionPoint> {
        let key = sorted(set);
        sessions
            .iter()
            .filter(|p| {
                p.valid && Some(&p.context) == latest_ctx.as_ref() && sorted(&p.tweak_set) == key
            })
            .collect()
    };

    let base = runs_of(&kept);
    if base.len() < s.min_runs {
        return Next::Run {
            tweaks: kept,
            testing: None,
        };
    }
    let Some(cur) = current else {
        return Next::Converged { kept };
    };
    let mut cand_set = kept.clone();
    cand_set.push(cur.clone());
    let cand = runs_of(&cand_set);
    if cand.len() < s.min_runs {
        return Next::Run {
            tweaks: cand_set,
            testing: Some(cur.clone()),
        };
    }
    let measure = measure_of(cur);
    let higher = !matches!(measure, Measure::BackgroundNoise | Measure::Pacing);
    let bv: Vec<f64> = base
        .iter()
        .map(|p| metric_value(&p.metrics, measure))
        .collect();
    let cv: Vec<f64> = cand
        .iter()
        .map(|p| metric_value(&p.metrics, measure))
        .collect();
    let mut c = compare(&bv, &cv, higher, s);
    if c.verdict == Verdict::Inconclusive {
        if cand.len() >= s.max_runs {
            c.verdict = Verdict::NoBenefit;
            c.exhausted = true;
        } else if base.len() < cand.len() {
            // Alternate A/B: growing only the candidate leaves the baseline at its minimum and
            // the confidence interval never narrows (few degrees of freedom).
            return Next::Run {
                tweaks: kept,
                testing: None,
            };
        } else {
            return Next::Run {
                tweaks: cand_set,
                testing: Some(cur.clone()),
            };
        }
    }
    Next::Decide {
        tweak: cur.clone(),
        comparison: c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> ExperimentSettings {
        ExperimentSettings::default()
    }

    #[test]
    fn clear_gain_is_kept() {
        let c = compare(&[40.0, 41.0, 39.5], &[52.0, 51.0, 53.0], true, &s());
        assert_eq!(c.verdict, Verdict::Kept, "{c:?}");
        assert!(c.delta_pct > 25.0);
    }

    #[test]
    fn clear_loss_is_harmful() {
        let c = compare(&[50.0, 50.5, 49.8], &[45.0, 45.5, 44.9], true, &s());
        assert_eq!(c.verdict, Verdict::Harmful, "{c:?}");
    }

    #[test]
    fn tight_null_effect_is_no_benefit() {
        let c = compare(
            &[50.0, 50.4, 49.9, 50.1],
            &[50.1, 49.8, 50.3, 50.0],
            true,
            &s(),
        );
        assert_eq!(c.verdict, Verdict::NoBenefit, "{c:?}");
    }

    #[test]
    fn noisy_small_effect_is_inconclusive() {
        let c = compare(&[45.0, 55.0], &[47.0, 57.0], true, &s());
        assert_eq!(c.verdict, Verdict::Inconclusive, "{c:?}");
    }

    #[test]
    fn lower_is_better_metrics() {
        // stutters/min dropping from ~6 to ~2 is an improvement
        let c = compare(&[6.0, 6.5, 5.8], &[2.0, 2.2, 1.9], false, &s());
        assert_eq!(c.verdict, Verdict::Kept, "{c:?}");
        assert!(c.delta_pct > 50.0);
        assert_eq!(
            compare(&[0.0, 0.05], &[0.0, 0.0], false, &s()).verdict,
            Verdict::NoBenefit
        );
    }

    #[test]
    fn t_table() {
        assert_eq!(t95(1.0), 12.706);
        assert_eq!(t95(2.7), 4.303);
        assert_eq!(t95(100.0), 1.96);
    }
}
