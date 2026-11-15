//! Hardware tier (goals.md §3). Deliberately simple and explainable.

use crate::profile::{HardwareProfile, Tier};

pub fn classify(p: &HardwareProfile) -> Tier {
    let ram = p.memory.total_gb.val();
    let cores = p.cpu.cores.val();
    match p.dedicated_gpu() {
        // No dedicated GPU but an integrated one: games are iGPU-limited whatever else is there.
        None if p.integrated_gpu().is_some() => Tier::Tier1,
        None => {
            // GPUs unknown: fall back on RAM/cores.
            if ram.is_some_and(|r| r <= 8.5) || cores.is_some_and(|c| c <= 4) {
                Tier::Tier2
            } else {
                Tier::Tier3
            }
        }
        Some(g) => {
            let small_vram = g.vram_mb.val().is_some_and(|v| v <= 4300);
            if small_vram || ram.is_some_and(|r| r <= 8.5) || cores.is_some_and(|c| c <= 4) {
                Tier::Tier2
            } else {
                Tier::Tier3
            }
        }
    }
}
