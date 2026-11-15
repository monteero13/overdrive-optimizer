//! overdrive-probe — runs the real hardware profiler on this machine and prints JSON.
//!
//!   overdrive-probe                 profile + ranked issues + BIOS Coach flows (pretty JSON)
//!   overdrive-probe --compact       single-line JSON
//!   overdrive-probe --stability 3   also count WHEA / crash / unexpected-shutdown events of the last N days
//!
//! Read-only: it never changes anything on the system.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use overdrive_core::source::StabilitySource;
use overdrive_core::{build_profile, coach, issues};
use overdrive_win::{UserHive, WinHardwareSource};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let compact = args.iter().any(|a| a == "--compact");
    let stability_days: Option<u64> = args
        .iter()
        .position(|a| a == "--stability")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    let t0 = Instant::now();
    let src = WinHardwareSource::new(UserHive::CurrentUser);
    let profile = build_profile(&src);
    let elapsed_ms = t0.elapsed().as_millis() as u64;

    let ranked = issues::structural_issues(&profile);
    let flows: Vec<_> = ranked
        .iter()
        .filter_map(|i| coach::flow_for(i, &profile))
        .collect();

    let mut out = json!({
        "durationMs": elapsed_ms,
        "withinBudget": elapsed_ms < 3000,
        "profile": profile,
        "issues": ranked,
        "coachFlows": flows,
    });

    if let Some(days) = stability_days {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
            .saturating_sub(days * 86_400_000);
        out["stability"] = match src.stability_events(since) {
            Ok(ev) => json!({ "days": days, "events": ev }),
            Err(e) => json!({ "days": days, "error": e.to_string() }),
        };
    }

    let s = if compact {
        serde_json::to_string(&out)
    } else {
        serde_json::to_string_pretty(&out)
    };
    println!("{}", s.expect("serialize"));
}
