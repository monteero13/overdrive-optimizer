//! Structural issues: what holds this PC back, ranked by impact — including things
//! software can't fix (goals.md §5.1 output). Ids and params match the UI i18n keys
//! (`issue.<id>.title/body`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::memspeed;
use crate::profile::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Impact {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FixPath {
    Software { tweak_id: String },
    Bios { flow_id: String },
    Hardware,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StructuralIssue {
    pub id: String,
    pub impact: Impact,
    pub confidence: Confidence,
    pub fixable_by: FixPath,
    pub params: BTreeMap<String, Value>,
}

fn issue(
    id: &str,
    impact: Impact,
    confidence: Confidence,
    fixable_by: FixPath,
    params: Value,
) -> StructuralIssue {
    let params = match params {
        Value::Object(m) => m.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    StructuralIssue {
        id: id.into(),
        impact,
        confidence,
        fixable_by,
        params,
    }
}

fn weakest(a: Confidence, b: Confidence) -> Confidence {
    a.max(b) // Exact < Inferred < Unknown
}

/// Thresholds (documented constants, goals.md §5.2 rule).
pub const SSD_FULL_FREE_PCT: f64 = 15.0; // > 85 % used
pub const THROTTLE_PERF_LIMIT_PCT: f64 = 85.0;
pub const DRIVER_OLD_DAYS: u32 = 365;
pub const UMA_SMALL_MB: u32 = 512;
pub const RAM_SPEED_TOLERANCE_MTS: u32 = 100;

pub fn structural_issues(p: &HardwareProfile) -> Vec<StructuralIssue> {
    let mut out = Vec::new();
    let laptop = p.is_laptop();

    // Power source
    if laptop && p.power.power_source.val() == Some(PowerSource::Battery) {
        out.push(issue(
            "on_battery",
            Impact::High,
            Confidence::Exact,
            FixPath::User,
            json!({}),
        ));
    }

    // RAM speed vs what the kit and platform allow
    let m = &p.memory;
    if let (Some(cfg), Some(rated)) = (m.configured_mts.val(), m.rated_mts.val()) {
        let cap = m.platform_cap_mts.val();
        let target = cap.map_or(rated, |c| rated.min(c));
        let conf = weakest(m.configured_mts.confidence, m.rated_mts.confidence);
        if cfg + RAM_SPEED_TOLERANCE_MTS < target {
            let mut params = json!({ "configured": cfg, "rated": rated, "target": target });
            if let Some(fb) = memspeed::step_below(target).filter(|&f| f > cfg) {
                params["fallback"] = json!(fb);
            }
            if let Some(c) = cap {
                params["cap"] = json!(c);
            }
            if laptop {
                // Laptop firmware almost never exposes XMP: say so instead of sending them to the BIOS.
                params["laptop"] = json!(true);
                out.push(issue(
                    "ram_below_rated",
                    Impact::Medium,
                    conf,
                    FixPath::User,
                    params,
                ));
            } else {
                out.push(issue(
                    "ram_below_rated",
                    Impact::High,
                    conf,
                    FixPath::Bios {
                        flow_id: "ram_xmp".into(),
                    },
                    params,
                ));
            }
        } else if let Some(c) = cap {
            if rated > c + RAM_SPEED_TOLERANCE_MTS && cfg + RAM_SPEED_TOLERANCE_MTS >= c {
                // Running at the platform maximum: nothing to fix in BIOS, but be honest about why.
                out.push(issue(
                    "ram_platform_locked",
                    Impact::Low,
                    Confidence::Inferred,
                    FixPath::Hardware,
                    json!({ "configured": cfg, "rated": rated, "cap": c, "chipset": p.firmware.chipset.get() }),
                ));
            }
        }
    }

    // Channels
    match (m.channels.val(), m.modules.len()) {
        (Some(1), 1) => {
            let gb = m.modules[0].capacity_gb.round();
            let free = m.slots_total.val().map(|t| t.saturating_sub(1));
            out.push(issue(
                "single_channel_ram",
                Impact::High,
                Confidence::Inferred,
                FixPath::Hardware,
                json!({ "gb": gb, "slotsFree": free }),
            ));
        }
        (Some(1), n) if n >= 2 => {
            out.push(issue(
                "ram_same_channel",
                Impact::High,
                Confidence::Inferred,
                FixPath::User,
                json!({ "modules": n }),
            ));
        }
        _ => {}
    }

    // Too little RAM
    if let Some(gb) = m.total_gb.val() {
        if gb < 7.5 {
            out.push(issue(
                "low_ram",
                Impact::High,
                Confidence::Exact,
                FixPath::Hardware,
                json!({ "gb": gb.round() }),
            ));
        }
    }

    // Storage
    if let Some(sys) = p.system_drive() {
        if sys.media.val() == Some(MediaType::Hdd) {
            out.push(issue(
                "system_on_hdd",
                Impact::High,
                Confidence::Exact,
                FixPath::Hardware,
                json!({ "model": sys.model }),
            ));
        }
    }
    for d in &p.storage {
        if matches!(
            d.health.val(),
            Some(DriveHealth::Warning | DriveHealth::Unhealthy)
        ) {
            out.push(issue(
                "drive_health",
                Impact::High,
                Confidence::Exact,
                FixPath::User,
                json!({ "model": d.model }),
            ));
        }
        if matches!(d.media.val(), Some(MediaType::SataSsd | MediaType::NvmeSsd)) {
            for v in d
                .volumes
                .iter()
                .filter(|v| v.free_pct < SSD_FULL_FREE_PCT && v.size_gb > 16.0)
            {
                out.push(issue(
                    "ssd_nearly_full",
                    Impact::Medium,
                    Confidence::Exact,
                    FixPath::User,
                    json!({ "letter": v.letter, "pct": (100.0 - v.free_pct).round() }),
                ));
            }
        }
    }

    // GPUs
    let dgpu = p.dedicated_gpu();
    let igpu = p.integrated_gpu();
    if dgpu.is_none() {
        if let Some(i) = igpu {
            if i.vendor == GpuVendor::Amd {
                if let Some(mb) = i.vram_mb.val().filter(|&mb| mb <= UMA_SMALL_MB) {
                    out.push(issue(
                        "uma_small",
                        Impact::Medium,
                        Confidence::Exact,
                        FixPath::Bios {
                            flow_id: "uma_buffer".into(),
                        },
                        json!({ "mb": mb }),
                    ));
                }
            }
        }
    }
    if laptop && dgpu.is_some() && igpu.is_some() {
        out.push(issue(
            "hybrid_gpu",
            Impact::Medium,
            Confidence::Exact,
            FixPath::Software {
                tweak_id: "hybrid_gpu".into(),
            },
            json!({ "dgpu": dgpu.map(|g| g.name.clone()), "igpu": igpu.map(|g| g.name.clone()) }),
        ));
    }
    if let Some(g) = dgpu {
        if let Some(l) = g.pcie.get() {
            if let (Some(cur), Some(max)) = (l.current_width, l.max_width) {
                if cur < max && max >= 8 {
                    out.push(issue(
                        "pcie_width_reduced",
                        Impact::Medium,
                        Confidence::Inferred,
                        FixPath::User,
                        json!({ "current": cur, "max": max, "gpu": g.name }),
                    ));
                }
            }
        }
        if let Some(days) = g.driver_age_days.val().filter(|&d| d > DRIVER_OLD_DAYS) {
            out.push(issue(
                "gpu_driver_old",
                Impact::Low,
                Confidence::Exact,
                FixPath::User,
                json!({ "days": days, "months": days / 30, "gpu": g.name }),
            ));
        }
    }

    // CPU throttling (single sample → inferred)
    if let Some(v) = p
        .cpu
        .perf_limit_pct
        .val()
        .filter(|&v| v < THROTTLE_PERF_LIMIT_PCT)
    {
        out.push(issue(
            if laptop {
                "thermal_hint"
            } else {
                "cpu_throttling"
            },
            Impact::Medium,
            Confidence::Inferred,
            FixPath::User,
            json!({ "limitPct": v.round() }),
        ));
    }

    // Windows settings with a software fix
    if p.windows.game_dvr.val() == Some(true) {
        out.push(issue(
            "game_dvr_on",
            Impact::Medium,
            Confidence::Exact,
            FixPath::Software {
                tweak_id: "gamedvr_off".into(),
            },
            json!({}),
        ));
    }
    if p.windows.game_mode.val() == Some(false) {
        out.push(issue(
            "game_mode_off",
            Impact::Low,
            Confidence::Exact,
            FixPath::Software {
                tweak_id: "game_mode".into(),
            },
            json!({}),
        ));
    }
    if p.windows.pending_reboot.val() == Some(true) {
        out.push(issue(
            "pending_reboot",
            Impact::Low,
            Confidence::Exact,
            FixPath::User,
            json!({}),
        ));
    }

    rank(&mut out);
    out
}

/// Impact first, then certainty, then id (stable output for tests and UI).
pub fn rank(v: &mut [StructuralIssue]) {
    v.sort_by(|a, b| {
        a.impact
            .cmp(&b.impact)
            .then(a.confidence.cmp(&b.confidence))
            .then(a.id.cmp(&b.id))
    });
}
