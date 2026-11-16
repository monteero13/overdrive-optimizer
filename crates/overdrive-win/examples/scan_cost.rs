//! How much does each part of the idle process watch cost? (footprint, goals.md §4.3)
//!   cargo run --release -p overdrive-win --example scan_cost
use std::time::Instant;

fn time<T>(name: &str, n: u32, mut f: impl FnMut() -> T) {
    let _ = f();
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(f());
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("{name:<28} {ms:>8.3} ms/call  → {:.3} % of a core at one call per 2 s", ms / 2000.0 * 100.0);
}

fn main() {
    use overdrive_win::processes;
    time("snapshot (NtQuerySystemInfo)", 50, || processes::snapshot().map(|v| v.len()));
    time("list (full scan)", 20, || processes::list().map(|v| v.len()));
}
