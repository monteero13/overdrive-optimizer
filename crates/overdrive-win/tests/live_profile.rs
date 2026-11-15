//! Runs the real profiler on the machine executing the tests (Windows 10/11).
//! Phase 3 DoD: valid profile, < 3 s, no kernel drivers, unknowns reported as unknown.

use std::time::Instant;

use overdrive_core::build_profile;
use overdrive_core::issues::structural_issues;
use overdrive_core::profile::Confidence;
use overdrive_win::{UserHive, WinHardwareSource};

#[test]
fn live_profile_is_complete_and_fast() {
    let t0 = Instant::now();
    let p = build_profile(&WinHardwareSource::new(UserHive::CurrentUser));
    let ms = t0.elapsed().as_millis();
    assert!(ms < 3000, "profile took {ms} ms (budget 3000)");
    assert!(p.warnings.is_empty(), "source failures: {:?}", p.warnings);

    // Core facts every Windows machine has.
    assert!(p.cpu.cores.val().unwrap_or(0) >= 1);
    assert!(p.cpu.threads.val().unwrap_or(0) >= p.cpu.cores.val().unwrap_or(0));
    assert!(p.memory.total_gb.val().unwrap_or(0.0) > 1.0);
    assert!(!p.memory.modules.is_empty() || p.memory.total_gb.is_known());
    assert!(
        !p.gpus.is_empty(),
        "DXGI should report at least one hardware adapter"
    );
    assert!(
        p.system_drive().is_some(),
        "the system drive must be identified"
    );
    assert!(
        p.windows.build.val().unwrap_or(0) >= 19041,
        "Windows 10 2004+ / Windows 11"
    );
    assert!(p.firmware.board_vendor.is_known());

    // Invariant: a field is unknown ⇔ it has no value; inferred fields always have one.
    let json = serde_json::to_value(&p).unwrap();
    fn walk(v: &serde_json::Value, path: &str) {
        if let Some(obj) = v.as_object() {
            if let (Some(c), Some(val)) = (obj.get("confidence"), obj.get("value")) {
                assert_eq!(
                    c == "unknown",
                    val.is_null(),
                    "inconsistent field at {path}: {v}"
                );
            }
            for (k, child) in obj {
                walk(child, &format!("{path}.{k}"));
            }
        } else if let Some(arr) = v.as_array() {
            for (i, child) in arr.iter().enumerate() {
                walk(child, &format!("{path}[{i}]"));
            }
        }
    }
    walk(&json, "profile");

    // Issues are computable and ranked; channel info is never claimed as exact.
    let _ = structural_issues(&p);
    assert_ne!(p.memory.channels.confidence, Confidence::Exact);
}
