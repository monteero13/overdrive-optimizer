//! Debug helper: per-window verdicts for one trace.  cargo run -p overdrive-core --example inspect_trace -- <name>
use overdrive_core::telemetry::classify::{classify, Thresholds};
use overdrive_core::telemetry::trace::{parse_presentmon_csv, parse_system_csv};
fn main() {
    let name = std::env::args().nth(1).expect("trace name");
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces");
    let f = parse_presentmon_csv(&std::fs::read_to_string(d.join(format!("{name}.csv"))).unwrap())
        .unwrap();
    let s = parse_system_csv(&std::fs::read_to_string(d.join(format!("{name}.sys.csv"))).unwrap())
        .unwrap();
    let c = classify(&f, &s, &Thresholds::default());
    for w in &c.windows {
        println!(
            "{:>7.0}-{:<7.0} {:?} raw={:?} smooth={:?} ratio={:?} stutters={}",
            w.range.start_ms,
            w.range.end_ms,
            w.kind,
            w.raw,
            w.smoothed,
            w.gpu_ratio.map(|r| (r * 100.0).round() / 100.0),
            w.stutters
        );
    }
    println!("=> {:?} {:.2}", c.label, c.confidence);
}
