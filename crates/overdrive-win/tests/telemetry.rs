//! Phase 4 on real Windows: the PresentMon streaming pipeline and the PDH system sampler,
//! including the overhead budget (goals.md §4.3: < 0.5 % of one core during a session).

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use overdrive_core::telemetry::classify::{classify, Thresholds};
use overdrive_core::telemetry::trace::{parse_presentmon_csv, parse_system_csv};
use overdrive_core::telemetry::{Bottleneck, TelemetrySource};
use overdrive_win::telemetry::{PresentMonSource, SystemSampler};

fn fixture(name: &str) -> PathBuf {
    // Native separators only: `cmd /C type` treats a '/' as a switch.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .join("overdrive-core")
        .join("tests")
        .join("traces")
        .join(name)
}

#[test]
fn presentmon_stdout_is_streamed_parsed_and_classified() {
    // Replay a recorded PresentMon 2.x CSV through the exact spawn → stdout → parser path.
    let path = fixture("cpu_bound_120fps.csv");
    let expected = parse_presentmon_csv(&std::fs::read_to_string(&path).unwrap())
        .unwrap()
        .len();
    let p = path.clone();
    let mut src = PresentMonSource::with_command(Box::new(move |_pid| {
        let mut c = Command::new("cmd");
        c.arg("/C").arg("type").arg(&p);
        c
    }));
    src.start(4242).unwrap();
    let t0 = Instant::now();
    let mut frames = Vec::new();
    while frames.len() < expected && t0.elapsed() < Duration::from_secs(20) {
        frames.extend(src.poll());
        std::thread::sleep(Duration::from_millis(20));
    }
    src.stop().unwrap();
    assert_eq!(frames.len(), expected, "every frame line must come through");
    let sys =
        parse_system_csv(&std::fs::read_to_string(fixture("cpu_bound_120fps.sys.csv")).unwrap())
            .unwrap();
    assert_eq!(
        classify(&frames, &sys, &Thresholds::default()).label,
        Bottleneck::CpuBound
    );
    assert!(src.stop().is_err(), "stop twice → NotRunning");
}

#[test]
fn missing_tool_is_a_clear_error() {
    let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("no-such-dir")
        .join("PresentMon.exe");
    let mut src = PresentMonSource::new(missing);
    assert!(src.start(1).is_err());
}

#[test]
fn system_sampler_reads_counters_within_budget() {
    let sampler = SystemSampler::start(Duration::from_millis(500));
    std::thread::sleep(Duration::from_millis(3200));
    let (samples, stats) = sampler.stop();

    assert!(samples.len() >= 5, "got {} samples", samples.len());
    let last = samples.last().unwrap();
    assert!(
        last.ram_commit_pct.is_some_and(|v| v > 0.0 && v <= 100.0),
        "{last:?}"
    );
    assert!(last.hard_faults_per_s.is_some(), "{last:?}");
    assert!(last.disk_queue.is_some(), "{last:?}");
    assert!(last.perf_limit_pct.is_some(), "{last:?}");
    assert!(last.on_battery.is_some(), "{last:?}");
    assert!(
        samples.windows(2).all(|w| w[1].t_ms > w[0].t_ms),
        "monotonic time"
    );

    // Budget: measured on the sampler thread only (other tests run in parallel in this
    // process). At 2 Hz — twice the production rate — steady state must stay < 0.5 % of a core.
    let pct = stats.steady_pct();
    println!("sampler: {} samples, setup CPU {:?} (one-time), steady {} cycles over {:?} = {pct:.4} % of one core; last = {last:?}", samples.len(), stats.setup_cpu, stats.steady_cycles, stats.steady_wall);
    assert!(pct < 0.5, "sampler overhead {pct:.3} % ≥ 0.5 % of one core");
    assert!(stats.steady_cycles > 0, "cycle counter must work");
    // One-time cost when a session starts (PDH loads the GPU counter provider).
    assert!(
        stats.setup_cpu < Duration::from_millis(1500),
        "PDH setup too heavy: {:?}",
        stats.setup_cpu
    );
}

#[test]
fn parsing_cost_is_negligible() {
    // A 300 FPS game produces 300 lines/s; parsing must cost a tiny fraction of that budget.
    let text = std::fs::read_to_string(fixture("cpu_bound_with_gpu_blip.csv")).unwrap();
    // Best of 5: the budget is about the parser, not about scheduler noise on a busy machine.
    let mut per_frame_us = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let frames = parse_presentmon_csv(&text).unwrap();
        per_frame_us = per_frame_us.min(t0.elapsed().as_secs_f64() * 1e6 / frames.len() as f64);
    }
    println!(
        "parse: {:.2} µs/frame → {:.4} % of one core at 300 FPS",
        per_frame_us,
        per_frame_us * 300.0 / 1e4
    );
    assert!(
        // Same budget as the sampler: < 0.5 % of one core (even in an unoptimized build).
        per_frame_us * 300.0 / 1e6 < 0.005,
        "{per_frame_us} µs/frame is too slow"
    );
}
