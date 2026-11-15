//! Deterministic synthetic session generator — the source of the reference trace library
//! in `tests/traces/` (regenerate with `cargo run -p overdrive-core --example gen_traces`).
//!
//! Each scenario reproduces the *signature* of a bottleneck as PresentMon and PDH would
//! see it (GPU Busy ratio, stutter timing, fault/disk bursts, perf-limit drops, loading
//! screens, menus). Real captures from friends' PCs are added to the same library later.

use crate::telemetry::{Bottleneck, FrameSample, SystemSample};

/// xorshift64* — tiny, deterministic, no dependency.
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64) / ((1u64 << 53) as f64)
    }
    /// Uniform in [-1, 1).
    pub fn sym(&mut self) -> f64 {
        self.next_f64() * 2.0 - 1.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// Loading screen: very long, irregular frames.
    Loading { secs: f64 },
    /// Menu: vsync-steady 60 FPS, GPU nearly idle.
    Menu { secs: f64 },
    /// Gameplay with a given GPU busy ratio.
    Play {
        secs: f64,
        frametime_ms: f64,
        gpu_ratio: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pressure {
    None,
    /// RAM commit at `pct`, with hard-fault bursts every `every_s` that cause a stutter.
    MemoryFaults {
        pct: f64,
        every_s: f64,
    },
    /// VRAM high but stutters are random (no fault correlation).
    VramHighOnly {
        pct: f64,
    },
    /// Disk queue bursts every `every_s` that cause a stutter.
    Disk {
        every_s: f64,
    },
    /// CPU perf limit drops to `to_pct` from `from_s` on (frametimes rise accordingly).
    Thermal {
        from_s: f64,
        to_pct: f64,
    },
    Battery,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: &'static str,
    pub seed: u64,
    pub phases: Vec<Phase>,
    pub pressure: Pressure,
    /// Random (uncorrelated) stutters per second of gameplay.
    pub random_stutters_per_s: f64,
    pub has_gpu_busy: bool,
    pub expected: Bottleneck,
    pub note: &'static str,
}

pub struct Session {
    pub frames: Vec<FrameSample>,
    pub system: Vec<SystemSample>,
}

pub fn generate(sc: &Scenario) -> Session {
    let mut rng = Rng::new(sc.seed);
    let mut frames = Vec::new();
    let mut t = 0.0f64;
    // Burst times (ms) for correlated pressure.
    let total_s: f64 = sc
        .phases
        .iter()
        .map(|p| match p {
            Phase::Loading { secs } | Phase::Menu { secs } | Phase::Play { secs, .. } => *secs,
        })
        .sum();
    let bursts: Vec<f64> = match sc.pressure {
        Pressure::MemoryFaults { every_s, .. } | Pressure::Disk { every_s } => {
            let mut v = Vec::new();
            let mut b = every_s * 1000.0 * (0.5 + rng.next_f64() * 0.5);
            while b < total_s * 1000.0 {
                v.push(b);
                b += every_s * 1000.0 * (0.8 + rng.next_f64() * 0.4);
            }
            v
        }
        _ => vec![],
    };
    let mut next_burst = 0usize;

    for phase in &sc.phases {
        match *phase {
            Phase::Loading { secs } => {
                let end = t + secs * 1000.0;
                while t < end {
                    let ft = 280.0 + rng.next_f64() * 400.0;
                    frames.push(FrameSample {
                        t_ms: t,
                        frametime_ms: ft,
                        gpu_busy_ms: sc.has_gpu_busy.then_some(ft * 0.05),
                        display_latency_ms: Some(ft + 8.0),
                    });
                    t += ft;
                }
            }
            Phase::Menu { secs } => {
                let end = t + secs * 1000.0;
                while t < end {
                    let ft = 16.667 + rng.sym() * 0.15;
                    frames.push(FrameSample {
                        t_ms: t,
                        frametime_ms: ft,
                        gpu_busy_ms: sc
                            .has_gpu_busy
                            .then_some(ft * (0.08 + rng.next_f64() * 0.04)),
                        display_latency_ms: Some(ft * 1.5),
                    });
                    t += ft;
                }
            }
            Phase::Play {
                secs,
                frametime_ms,
                gpu_ratio,
            } => {
                let end = t + secs * 1000.0;
                let mut stutter_debt = 0.0;
                while t < end {
                    let mut base = frametime_ms;
                    let mut ratio = gpu_ratio;
                    if let Pressure::Thermal { from_s, to_pct } = sc.pressure {
                        if t >= from_s * 1000.0 {
                            // Clocks drop → CPU-side work takes longer.
                            base *= 100.0 / to_pct;
                            ratio *= to_pct / 100.0;
                        }
                    }
                    let mut ft = base * (1.0 + rng.sym() * 0.06 + (t / 900.0).sin() * 0.03);
                    let mut spike = false;
                    if next_burst < bursts.len() && t >= bursts[next_burst] {
                        next_burst += 1;
                        spike = true;
                    }
                    stutter_debt += sc.random_stutters_per_s * ft / 1000.0;
                    if stutter_debt >= 1.0 && rng.next_f64() < 0.5 {
                        stutter_debt -= 1.0;
                        spike = true;
                    }
                    if spike {
                        ft = base * (3.0 + rng.next_f64()) + 10.0;
                        ratio = (base * ratio) / ft; // GPU waits during the hitch
                    }
                    let gpu = (ft * ratio * (1.0 + rng.sym() * 0.02)).min(ft);
                    frames.push(FrameSample {
                        t_ms: t,
                        frametime_ms: ft,
                        gpu_busy_ms: sc.has_gpu_busy.then_some(gpu),
                        display_latency_ms: Some(ft * 1.6),
                    });
                    t += ft;
                }
            }
        }
    }

    // 1 Hz system samples.
    let mut system = Vec::new();
    let mut s = 1000.0;
    while s <= t + 1000.0 {
        let near_burst = bursts.iter().any(|&b| b <= s && s - b < 1000.0);
        let mut x = SystemSample {
            t_ms: s,
            ram_commit_pct: Some(58.0 + rng.sym() * 3.0),
            vram_usage_pct: Some(62.0 + rng.sym() * 4.0),
            hard_faults_per_s: Some(5.0 + rng.next_f64() * 20.0),
            disk_queue: Some(0.1 + rng.next_f64() * 0.3),
            perf_limit_pct: Some(100.0),
            on_battery: Some(false),
        };
        match sc.pressure {
            Pressure::MemoryFaults { pct, .. } => {
                x.ram_commit_pct = Some(pct + rng.sym());
                if near_burst {
                    x.hard_faults_per_s = Some(600.0 + rng.next_f64() * 900.0);
                }
            }
            Pressure::VramHighOnly { pct } => x.vram_usage_pct = Some(pct + rng.sym()),
            Pressure::Disk { .. } => {
                if near_burst {
                    x.disk_queue = Some(4.0 + rng.next_f64() * 6.0);
                }
            }
            Pressure::Thermal { from_s, to_pct } => {
                if s >= from_s * 1000.0 {
                    x.perf_limit_pct = Some(to_pct + rng.sym() * 2.0);
                }
            }
            Pressure::Battery => x.on_battery = Some(true),
            Pressure::None => {}
        }
        system.push(x);
        s += 1000.0;
    }
    Session { frames, system }
}

/// The reference library. Expected labels are what a careful human would conclude.
pub fn library() -> Vec<Scenario> {
    use Bottleneck::*;
    let play = |secs, frametime_ms, gpu_ratio| Phase::Play {
        secs,
        frametime_ms,
        gpu_ratio,
    };
    vec![
        Scenario {
            name: "gpu_bound_60fps",
            seed: 11,
            phases: vec![play(45.0, 16.7, 0.97)],
            pressure: Pressure::None,
            random_stutters_per_s: 0.05,
            has_gpu_busy: true,
            expected: GpuBound,
            note: "GPU busy ≈ frametime the whole session",
        },
        Scenario {
            name: "cpu_bound_120fps",
            seed: 12,
            phases: vec![play(40.0, 8.3, 0.52)],
            pressure: Pressure::None,
            random_stutters_per_s: 0.1,
            has_gpu_busy: true,
            expected: CpuBound,
            note: "GPU idles ~half of every frame",
        },
        Scenario {
            name: "mixed_boundary",
            seed: 13,
            phases: vec![play(40.0, 12.0, 0.83)],
            pressure: Pressure::None,
            random_stutters_per_s: 0.05,
            has_gpu_busy: true,
            expected: Mixed,
            note: "Ratio between the CPU and GPU thresholds",
        },
        Scenario {
            name: "memory_bound_8gb",
            seed: 14,
            phases: vec![play(45.0, 18.0, 0.8)],
            pressure: Pressure::MemoryFaults {
                pct: 94.0,
                every_s: 1.6,
            },
            random_stutters_per_s: 0.0,
            has_gpu_busy: true,
            expected: MemoryBound,
            note: "RAM 94 % and every stutter lines up with a hard-fault burst",
        },
        Scenario {
            name: "vram_high_but_cpu_bound",
            seed: 15,
            phases: vec![play(40.0, 9.0, 0.55)],
            pressure: Pressure::VramHighOnly { pct: 95.0 },
            random_stutters_per_s: 0.4,
            has_gpu_busy: true,
            expected: CpuBound,
            note: "High memory use alone is NOT a bottleneck: stutters don't correlate",
        },
        Scenario {
            name: "io_bound_hdd",
            seed: 16,
            phases: vec![play(45.0, 15.0, 0.86)],
            pressure: Pressure::Disk { every_s: 1.6 },
            random_stutters_per_s: 0.0,
            has_gpu_busy: true,
            expected: IoBound,
            note: "Stutters line up with disk-queue bursts (asset streaming from HDD)",
        },
        Scenario {
            name: "thermal_laptop",
            seed: 17,
            phases: vec![play(60.0, 14.0, 0.7)],
            pressure: Pressure::Thermal {
                from_s: 12.0,
                to_pct: 68.0,
            },
            random_stutters_per_s: 0.05,
            has_gpu_busy: true,
            expected: ThermalOrPowerLimited,
            note: "CPU perf limit drops to ~68 % after 12 s and stays there",
        },
        Scenario {
            name: "battery_laptop",
            seed: 18,
            phases: vec![play(30.0, 20.0, 0.9)],
            pressure: Pressure::Battery,
            random_stutters_per_s: 0.05,
            has_gpu_busy: true,
            expected: ThermalOrPowerLimited,
            note: "Playing on battery",
        },
        Scenario {
            name: "loading_menu_then_gpu",
            seed: 19,
            phases: vec![
                Phase::Loading { secs: 8.0 },
                Phase::Menu { secs: 15.0 },
                play(45.0, 16.7, 0.95),
            ],
            pressure: Pressure::None,
            random_stutters_per_s: 0.05,
            has_gpu_busy: true,
            expected: GpuBound,
            note: "Loading screen and menu must be excluded, not counted as stutter or CPU-bound",
        },
        Scenario {
            name: "cpu_bound_with_gpu_blip",
            seed: 20,
            phases: vec![
                play(25.0, 8.0, 0.5),
                play(5.0, 10.0, 0.97),
                play(25.0, 8.0, 0.5),
            ],
            pressure: Pressure::None,
            random_stutters_per_s: 0.05,
            has_gpu_busy: true,
            expected: CpuBound,
            note: "A 5 s GPU-heavy scene must not flip the verdict (hysteresis)",
        },
        Scenario {
            name: "no_gpu_busy_v1",
            seed: 21,
            phases: vec![play(30.0, 16.7, 0.9)],
            pressure: Pressure::None,
            random_stutters_per_s: 0.05,
            has_gpu_busy: false,
            expected: Unknown,
            note: "Old PresentMon without GPU busy: say unknown, don't guess",
        },
    ]
}
