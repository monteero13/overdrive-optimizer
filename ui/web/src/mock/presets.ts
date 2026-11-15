// Three representative machines from goals.md §3 (one per tier).
import type { GameCard, HardwareSummary, StructuralIssue } from '../proto/types.ts';

export type PresetId = 'laptop_hybrid' | 'desktop_entry' | 'apu_lowend';

export interface Preset {
  id: PresetId;
  hardware: HardwareSummary;
  issues: StructuralIssue[];
  hybridGpu: boolean;
  nvidia: boolean;
  lowRam: boolean;
  baseFps: number; // typical fps for a mid game before fixes
}

const ex = <T>(value: T) => ({ value, confidence: 'exact' as const });
const inf = <T>(value: T) => ({ value, confidence: 'inferred' as const });

export const PRESETS: Record<PresetId, Preset> = {
  laptop_hybrid: {
    id: 'laptop_hybrid',
    hybridGpu: true,
    nvidia: true,
    lowRam: true,
    baseFps: 54,
    hardware: {
      tier: 3,
      isLaptop: true,
      powerSource: 'ac',
      os: 'Windows 11 24H2',
      cpu: ex('AMD Ryzen 5 5600H · 6C/12T'),
      gpu: ex('NVIDIA GeForce GTX 1650 · 4 GB'),
      gpuSecondary: ex('AMD Radeon Graphics (integrada)'),
      vramGb: ex(4),
      ramGb: ex(8),
      ramSpeed: ex({ configured: 3200, rated: 3200 }),
      ramChannels: inf(1),
      storage: ex('NVMe SSD 512 GB'),
      storageFreePct: ex(31),
      boardVendor: ex('Lenovo'),
    },
    issues: [
      { id: 'game_on_igpu', impact: 'high', confidence: 'exact', fixableBy: { kind: 'software', tweakId: 'hybrid_gpu' }, params: { game: 'Fortnite' } },
      { id: 'single_channel_ram', impact: 'high', confidence: 'inferred', fixableBy: { kind: 'hardware' }, params: { gb: 8 } },
      { id: 'thermal_hint', impact: 'low', confidence: 'inferred', fixableBy: { kind: 'user' } },
    ],
  },
  desktop_entry: {
    id: 'desktop_entry',
    hybridGpu: false,
    nvidia: true,
    lowRam: false,
    baseFps: 71,
    hardware: {
      tier: 2,
      isLaptop: false,
      powerSource: 'ac',
      os: 'Windows 10 22H2',
      cpu: ex('Intel Core i3-10100F · 4C/8T'),
      gpu: ex('NVIDIA GeForce GTX 1050 Ti · 4 GB'),
      gpuSecondary: null,
      vramGb: ex(4),
      ramGb: ex(16),
      ramSpeed: ex({ configured: 2133, rated: 3200 }),
      ramChannels: inf(2),
      storage: ex('SATA SSD 480 GB'),
      storageFreePct: ex(9),
      boardVendor: ex('MSI'),
    },
    issues: [
      { id: 'ram_below_rated', impact: 'high', confidence: 'exact', fixableBy: { kind: 'bios', flowId: 'ram_xmp' }, params: { configured: 2133, rated: 3200 } },
      { id: 'ssd_nearly_full', impact: 'medium', confidence: 'exact', fixableBy: { kind: 'user' }, params: { pct: 91 } },
    ],
  },
  apu_lowend: {
    id: 'apu_lowend',
    hybridGpu: false,
    nvidia: false,
    lowRam: true,
    baseFps: 38,
    hardware: {
      tier: 1,
      isLaptop: false,
      powerSource: 'ac',
      os: 'Windows 10 22H2',
      cpu: ex('AMD Ryzen 3 3200G · 4C/4T'),
      gpu: ex('AMD Radeon Vega 8 (integrada)'),
      gpuSecondary: null,
      vramGb: ex(0.5),
      ramGb: ex(8),
      ramSpeed: ex({ configured: 2400, rated: 3000 }),
      ramChannels: ex(2),
      storage: ex('HDD 1 TB'),
      storageFreePct: ex(44),
      boardVendor: ex('ASUS'),
    },
    issues: [
      { id: 'ram_below_rated', impact: 'high', confidence: 'exact', fixableBy: { kind: 'bios', flowId: 'ram_xmp' }, params: { configured: 2400, rated: 3000 } },
      { id: 'uma_small', impact: 'medium', confidence: 'exact', fixableBy: { kind: 'bios', flowId: 'uma_buffer' }, params: { mb: 512 } },
      { id: 'game_on_hdd', impact: 'medium', confidence: 'exact', fixableBy: { kind: 'hardware' } },
    ],
  },
};

export function gamesFor(p: Preset): GameCard[] {
  const gpuLimited = p.id === 'apu_lowend' ? 'gpu_bound' : 'memory_bound';
  return [
    { id: 'epic:fortnite', name: 'Fortnite', launcher: 'epic', antiCheat: 'kernel', antiCheatName: 'Easy Anti-Cheat', bottleneck: p.hybridGpu ? 'gpu_bound' : 'cpu_bound', bottleneckParams: p.hybridGpu ? { detail: 'igpu' } : {}, status: 'ready', fixesReady: 0, measured: null, hue: 265 },
    { id: 'riot:valorant', name: 'Valorant', launcher: 'riot', antiCheat: 'kernel', antiCheatName: 'Vanguard', bottleneck: 'cpu_bound', status: 'needs_baseline', fixesReady: 0, measured: null, hue: 352 },
    { id: 'steam:730', name: 'Counter-Strike 2', launcher: 'steam', antiCheat: 'user_mode', antiCheatName: 'VAC', bottleneck: 'cpu_bound', status: 'ready', fixesReady: 0, measured: null, hue: 38 },
    { id: 'steam:1245620', name: 'Elden Ring', launcher: 'steam', antiCheat: 'kernel', antiCheatName: 'Easy Anti-Cheat', bottleneck: gpuLimited, bottleneckParams: { vram: p.hardware.vramGb.value ?? 0 }, status: 'ready', fixesReady: 0, measured: null, hue: 48 },
    { id: 'xbox:minecraft', name: 'Minecraft', launcher: 'xbox', antiCheat: 'none', antiCheatName: null, bottleneck: 'cpu_bound', status: 'ready', fixesReady: 0, measured: null, hue: 140 },
    { id: 'steam:367520', name: 'Hollow Knight', launcher: 'steam', antiCheat: 'none', antiCheatName: null, bottleneck: 'thermal_or_power_limited', status: 'needs_baseline', fixesReady: 0, measured: null, hue: 210 },
  ];
}
