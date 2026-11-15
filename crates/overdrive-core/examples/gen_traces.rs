//! Regenerates the reference trace library in `crates/overdrive-core/tests/traces/`.
//!   cargo run -p overdrive-core --example gen_traces
use std::fs;
use std::path::Path;

use overdrive_core::synth::{generate, library};
use overdrive_core::telemetry::trace::{to_presentmon_v1_csv, to_presentmon_v2_csv, to_system_csv};

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("traces");
    fs::create_dir_all(&dir).unwrap();
    let mut manifest = Vec::new();
    for sc in library() {
        let s = generate(&sc);
        let v1 = !sc.has_gpu_busy;
        let csv = if v1 {
            to_presentmon_v1_csv("game.exe", 4242, &s.frames)
        } else {
            to_presentmon_v2_csv("game.exe", 4242, &s.frames)
        };
        fs::write(dir.join(format!("{}.csv", sc.name)), csv).unwrap();
        fs::write(
            dir.join(format!("{}.sys.csv", sc.name)),
            to_system_csv(&s.system),
        )
        .unwrap();
        manifest.push(serde_json::json!({
            "name": sc.name,
            "format": if v1 { "presentmon-1.x" } else { "presentmon-2.x" },
            "expected": sc.expected,
            "origin": "synthetic",
            "seed": sc.seed,
            "note": sc.note,
        }));
        println!(
            "{:<26} {:>6} frames  expected {:?}",
            sc.name,
            s.frames.len(),
            sc.expected
        );
    }
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap() + "\n",
    )
    .unwrap();
}
