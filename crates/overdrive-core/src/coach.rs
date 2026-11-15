//! BIOS Coach (goals.md §5.3b): detect → explain → guide (vendor-specific) →
//! verify after reboot → watch stability → measure. Software never changes firmware;
//! it only guides, verifies and watches.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::issues::{FixPath, StructuralIssue};
use crate::memspeed;
use crate::profile::*;
use crate::source::RawStabilityEvents;

pub const STABILITY_WATCH_DAYS: u32 = 3;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlowId {
    RamXmp,
    UmaBuffer,
}

impl FlowId {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ram_xmp" => Some(Self::RamXmp),
            "uma_buffer" => Some(Self::UmaBuffer),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::RamXmp => "ram_xmp",
            Self::UmaBuffer => "uma_buffer",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Explain,
    Reboot,
    Instruction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoachStep {
    pub kind: StepKind,
    /// i18n key, e.g. `coach.ram_xmp.where.msi`.
    pub key: String,
    pub params: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoachFlow {
    pub flow_id: FlowId,
    pub vendor: BoardVendor,
    /// Value before the change (MT/s for RAM, MB for UMA) — used by verification.
    pub before: u32,
    /// What we are aiming for.
    pub target: u32,
    /// "One step below" if the target turns out unstable.
    pub fallback: Option<u32>,
    pub steps: Vec<CoachStep>,
}

fn step(kind: StepKind, key: impl Into<String>, params: Value) -> CoachStep {
    let params = match params {
        Value::Object(m) => m.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    CoachStep {
        kind,
        key: key.into(),
        params,
    }
}

fn vendor_key(v: BoardVendor) -> &'static str {
    match v {
        BoardVendor::Msi => "msi",
        BoardVendor::Asus => "asus",
        BoardVendor::Gigabyte => "gigabyte",
        BoardVendor::Asrock => "asrock",
        BoardVendor::Lenovo => "lenovo",
        BoardVendor::Hp => "hp",
        BoardVendor::Dell => "dell",
        BoardVendor::Acer => "acer",
        BoardVendor::Generic => "generic",
    }
}

/// What the memory profile setting is called on this board.
fn profile_name(v: BoardVendor, cpu: Option<CpuVendor>) -> &'static str {
    let amd = cpu == Some(CpuVendor::Amd);
    match (v, amd) {
        (BoardVendor::Asus, true) => "DOCP / EXPO",
        (BoardVendor::Msi, true) => "A-XMP / EXPO",
        (BoardVendor::Msi, false) => "XMP",
        (BoardVendor::Asus | BoardVendor::Gigabyte | BoardVendor::Asrock, false) => "XMP",
        (BoardVendor::Gigabyte | BoardVendor::Asrock, true) => "XMP / EXPO",
        _ => "XMP / EXPO / DOCP",
    }
}

/// Build the guided flow for a BIOS-fixable issue.
pub fn flow_for(issue: &StructuralIssue, p: &HardwareProfile) -> Option<CoachFlow> {
    let FixPath::Bios { flow_id } = &issue.fixable_by else {
        return None;
    };
    let flow = FlowId::parse(flow_id)?;
    let vendor = p
        .firmware
        .board_vendor
        .val()
        .unwrap_or(BoardVendor::Generic);
    let vk = vendor_key(vendor);
    // OEM desktops (Lenovo/HP/Dell/Acer) rarely expose these menus with stable names.
    let menu_vk = match vendor {
        BoardVendor::Msi | BoardVendor::Asus | BoardVendor::Gigabyte | BoardVendor::Asrock => vk,
        _ => "generic",
    };
    let enter = step(
        StepKind::Instruction,
        format!("coach.vendor.{vk}.enter"),
        json!({}),
    );
    let reboot = step(StepKind::Reboot, "coach.reboot", json!({}));
    let save = step(StepKind::Instruction, "coach.save_exit", json!({}));

    match flow {
        FlowId::RamXmp => {
            let before = p.memory.configured_mts.val()?;
            let target = issue
                .params
                .get("target")
                .and_then(Value::as_u64)
                .map(|v| v as u32)?;
            let fallback = memspeed::step_below(target).filter(|&f| f > before);
            let name = profile_name(vendor, p.cpu.vendor.val());
            Some(CoachFlow {
                flow_id: flow,
                vendor,
                before,
                target,
                fallback,
                steps: vec![
                    step(
                        StepKind::Explain,
                        "coach.ram_xmp.explain",
                        json!({ "configured": before, "rated": issue.params.get("rated"), "target": target }),
                    ),
                    reboot,
                    enter,
                    step(
                        StepKind::Instruction,
                        format!("coach.ram_xmp.where.{menu_vk}"),
                        json!({ "profile": name }),
                    ),
                    step(
                        StepKind::Instruction,
                        "coach.ram_xmp.set",
                        json!({ "profile": name, "target": target, "fallback": fallback }),
                    ),
                    save,
                ],
            })
        }
        FlowId::UmaBuffer => {
            let before = issue
                .params
                .get("mb")
                .and_then(Value::as_u64)
                .map(|v| v as u32)?;
            // Memory given to the iGPU is taken from system RAM: be conservative on 8 GB.
            let target = if p.memory.total_gb.val().unwrap_or(8.0) >= 15.0 {
                2048
            } else {
                1024
            };
            Some(CoachFlow {
                flow_id: flow,
                vendor,
                before,
                target,
                fallback: Some(1024).filter(|&f| f < target && f > before),
                steps: vec![
                    step(
                        StepKind::Explain,
                        "coach.uma_buffer.explain",
                        json!({ "mb": before, "target": target }),
                    ),
                    reboot,
                    enter,
                    step(
                        StepKind::Instruction,
                        format!("coach.uma_buffer.where.{menu_vk}"),
                        json!({}),
                    ),
                    step(
                        StepKind::Instruction,
                        "coach.uma_buffer.set",
                        json!({ "target": target }),
                    ),
                    save,
                ],
            })
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifyStatus {
    /// Running at the target.
    Applied,
    /// Faster than before but below target (e.g. the BIOS picked a lower profile).
    Partial,
    NotApplied,
    /// The profile after reboot lacks the needed field.
    CannotVerify,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    pub status: VerifyStatus,
    pub before: u32,
    pub after: Option<u32>,
    pub target: u32,
}

/// Compare the fresh profile (after the reboot) against the flow.
pub fn verify_after_reboot(flow: &CoachFlow, after: &HardwareProfile) -> Verification {
    let now = match flow.flow_id {
        FlowId::RamXmp => after.memory.configured_mts.val(),
        FlowId::UmaBuffer => after.integrated_gpu().and_then(|g| g.vram_mb.val()),
    };
    let tol = match flow.flow_id {
        FlowId::RamXmp => 50,
        FlowId::UmaBuffer => 64,
    };
    let status = match now {
        None => VerifyStatus::CannotVerify,
        Some(v) if v + tol >= flow.target => VerifyStatus::Applied,
        Some(v) if v > flow.before + tol => VerifyStatus::Partial,
        Some(_) => VerifyStatus::NotApplied,
    };
    Verification {
        status,
        before: flow.before,
        after: now,
        target: flow.target,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StabilityVerdict {
    pub stable: bool,
    pub events: RawStabilityEvents,
    /// i18n key with what to do next.
    pub advice_key: String,
    pub fallback: Option<u32>,
}

/// Any hardware error, crash or unexpected shutdown during the watch window means
/// "go one step down" (or back to defaults if there is no lower step).
pub fn assess_stability(flow: &CoachFlow, events: RawStabilityEvents) -> StabilityVerdict {
    let unstable =
        events.whea_errors > 0 || events.bugchecks > 0 || events.unexpected_shutdowns > 0;
    let advice_key = match (unstable, flow.fallback) {
        (false, _) => "coach.stable",
        (true, Some(_)) => "coach.unstable.step_down",
        (true, None) => "coach.unstable.load_defaults",
    };
    StabilityVerdict {
        stable: !unstable,
        events,
        advice_key: advice_key.into(),
        fallback: if unstable { flow.fallback } else { None },
    }
}
