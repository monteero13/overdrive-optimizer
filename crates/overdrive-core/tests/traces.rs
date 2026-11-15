//! Phase 4 DoD: the classifier matches the expected label on every recorded trace.
//! Traces are read from disk (exercising the PresentMon parser), not regenerated.

use std::fs;
use std::path::PathBuf;

use overdrive_core::synth::{generate, library};
use overdrive_core::telemetry::classify::{classify, Thresholds, WindowKind};
use overdrive_core::telemetry::metrics::compute;
use overdrive_core::telemetry::trace::{parse_presentmon_csv, parse_system_csv};
use overdrive_core::telemetry::Bottleneck;
use serde::Deserialize;

#[derive(Deserialize)]
struct Entry {
    name: String,
    expected: Bottleneck,
    note: String,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("traces")
}

fn load(
    name: &str,
) -> (
    Vec<overdrive_core::telemetry::FrameSample>,
    Vec<overdrive_core::telemetry::SystemSample>,
) {
    let f = parse_presentmon_csv(&fs::read_to_string(dir().join(format!("{name}.csv"))).unwrap())
        .unwrap();
    let s = parse_system_csv(&fs::read_to_string(dir().join(format!("{name}.sys.csv"))).unwrap())
        .unwrap();
    (f, s)
}

#[test]
fn every_recorded_trace_gets_its_expected_label() {
    let manifest: Vec<Entry> =
        serde_json::from_str(&fs::read_to_string(dir().join("manifest.json")).unwrap()).unwrap();
    assert!(manifest.len() >= 10, "library too small");
    let th = Thresholds::default();
    let mut failures = Vec::new();
    for e in &manifest {
        let (frames, sys) = load(&e.name);
        let c = classify(&frames, &sys, &th);
        println!(
            "{:<26} → {:?} ({:.0} %)  expected {:?}",
            e.name,
            c.label,
            c.confidence * 100.0,
            e.expected
        );
        if c.label != e.expected {
            failures.push(format!(
                "{}: got {:?}, expected {:?} — {}",
                e.name, c.label, e.expected, e.note
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn trace_files_match_the_generator() {
    // Guards against editing the generator without regenerating the fixtures.
    for sc in library() {
        let (frames, _) = load(sc.name);
        let fresh = generate(&sc);
        assert_eq!(
            frames.len(),
            fresh.frames.len(),
            "{}: regenerate with `cargo run -p overdrive-core --example gen_traces`",
            sc.name
        );
    }
}

#[test]
fn loading_and_menu_are_excluded_from_metrics() {
    let (frames, sys) = load("loading_menu_then_gpu");
    let c = classify(&frames, &sys, &Thresholds::default());
    assert!(c.windows.iter().any(|w| w.kind == WindowKind::Loading));
    assert!(c.windows.iter().any(|w| w.kind == WindowKind::Menu));
    let excluded_ms: f64 = c.non_gameplay.iter().map(|r| r.end_ms - r.start_ms).sum();
    assert!(
        (18_000.0..=28_000.0).contains(&excluded_ms),
        "≈ 8 s loading + 15 s menu excluded, got {excluded_ms} ms"
    );

    let with = compute(&frames, &c.non_gameplay);
    let without = compute(&frames, &[]);
    assert!(
        with.low_01 > without.low_01 * 3.0,
        "loading frames must not count as stutter: {} vs {}",
        with.low_01,
        without.low_01
    );
    assert!(with.stutters < without.stutters || without.stutters == with.stutters);
    assert!(
        (with.avg_fps - 60.0).abs() < 4.0,
        "gameplay ≈ 60 FPS, got {}",
        with.avg_fps
    );
}

#[test]
fn hysteresis_keeps_a_short_blip_from_flipping_the_timeline() {
    let (frames, sys) = load("cpu_bound_with_gpu_blip");
    let c = classify(&frames, &sys, &Thresholds::default());
    let raw_gpu = c
        .windows
        .iter()
        .filter(|w| w.raw == Some(Bottleneck::GpuBound))
        .count();
    assert!(raw_gpu >= 1, "the blip is visible in raw labels");
    assert!(
        c.windows
            .iter()
            .filter_map(|w| w.smoothed)
            .all(|l| l == Bottleneck::CpuBound),
        "but never in the smoothed timeline"
    );
    assert!(c.confidence >= 0.99);
}

#[test]
fn memory_pressure_alone_is_not_memory_bound() {
    let (frames, sys) = load("vram_high_but_cpu_bound");
    assert!(sys.iter().all(|s| s.vram_usage_pct.unwrap() > 90.0));
    assert_eq!(
        classify(&frames, &sys, &Thresholds::default()).label,
        Bottleneck::CpuBound
    );
}

#[test]
fn evidence_is_reported() {
    let (frames, sys) = load("gpu_bound_60fps");
    let c = classify(&frames, &sys, &Thresholds::default());
    let ratio = c
        .evidence
        .iter()
        .find(|e| e.id == "gpu_busy_ratio")
        .unwrap()
        .value;
    assert!(ratio >= 0.9, "{ratio}");
    assert!(c
        .evidence
        .iter()
        .any(|e| e.id == "gameplay_windows" && e.value >= 8.0));
}

#[test]
fn an_isolated_hitch_is_a_stutter_not_a_loading_screen() {
    let (mut frames, sys) = load("gpu_bound_60fps");
    let before = compute(&frames, &[]).stutters;
    let i = frames.len() / 2;
    let shift = 320.0 - frames[i].frametime_ms;
    frames[i].frametime_ms = 320.0;
    for f in &mut frames[i + 1..] {
        f.t_ms += shift;
    }
    let c = classify(&frames, &sys, &Thresholds::default());
    assert!(
        c.non_gameplay.is_empty(),
        "a single 320 ms hitch must stay visible: {:?}",
        c.non_gameplay
    );
    // One frame in ~2700 doesn't move P99.9 (by definition), but it IS counted as a stutter.
    let m = compute(&frames, &c.non_gameplay);
    assert_eq!(m.stutters, before + 1, "the hitch is counted as a stutter");
    assert_eq!(c.label, Bottleneck::GpuBound);
}

#[test]
fn degenerate_inputs_are_unknown_not_panics() {
    let th = Thresholds::default();
    assert_eq!(classify(&[], &[], &th).label, Bottleneck::Unknown);
    let (frames, _) = load("gpu_bound_60fps");
    assert_eq!(
        classify(&frames[..10], &[], &th).label,
        Bottleneck::Unknown,
        "too few frames"
    );
    assert_ne!(
        classify(&frames, &[], &th).label,
        Bottleneck::Unknown,
        "works without system samples"
    );
}
