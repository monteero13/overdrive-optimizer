// Mock subset of catalog/*.toml (goals.md §5.3). Same fields the real catalog will have.
import type { Bottleneck, Evidence, Risk, Scope } from '../proto/types.ts';

export interface MockTweak {
  id: string;
  scope: Scope;
  risk: Risk;
  evidence: Evidence;
  requiresReboot: boolean;
  anticheatSafe: boolean;
  technical: string;
  appliesTo: Bottleneck[] | 'all';
  needs?: 'hybrid_gpu' | 'nvidia' | 'low_ram';
  /** Simulated true effect on this mock PC (% change of 1% low). Unknown to the UI until measured. */
  trueEffect: number;
  persistentCandidate: boolean;
}

export const CATALOG: MockTweak[] = [
  {
    id: 'hybrid_gpu',
    scope: 'persistent',
    risk: 'low',
    evidence: 'proven',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'HKCU\\Software\\Microsoft\\DirectX\\UserGpuPreferences  <exe> = "GpuPreference=2;"',
    appliesTo: 'all',
    needs: 'hybrid_gpu',
    trueEffect: 38,
    persistentCandidate: true,
  },
  {
    id: 'frame_cap',
    scope: 'session',
    risk: 'low',
    evidence: 'proven',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'NVAPI DRS · FRL_FPS (Max Frame Rate) = sustainable 1% low − 3 %',
    appliesTo: ['cpu_bound', 'mixed', 'thermal_or_power_limited'],
    trueEffect: 14,
    persistentCandidate: true,
  },
  {
    id: 'gamedvr_off',
    scope: 'persistent',
    risk: 'low',
    evidence: 'proven',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'HKCU\\System\\GameConfigStore  GameDVR_Enabled = 0 · AppCaptureEnabled = 0',
    appliesTo: 'all',
    trueEffect: 6,
    persistentCandidate: true,
  },
  {
    id: 'background_noise',
    scope: 'persistent',
    risk: 'low',
    evidence: 'proven',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'DiagTrack → Manual · Tasks: Application Experience\\*, Customer Experience Improvement Program\\* → Disabled',
    appliesTo: 'all',
    trueEffect: 4,
    persistentCandidate: true,
  },
  {
    id: 'power_session',
    scope: 'session',
    risk: 'low',
    evidence: 'proven',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'PowerSetActiveOverlayScheme(ded574b5-45a0-4f42-8737-46345c09c238) — only on AC',
    appliesTo: 'all',
    trueEffect: 5,
    persistentCandidate: false,
  },
  {
    id: 'pause_updates',
    scope: 'session',
    risk: 'low',
    evidence: 'proven',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'Windows Update active hours + Defender scheduled scan deferred while the game runs',
    appliesTo: 'all',
    trueEffect: 2,
    persistentCandidate: false,
  },
  {
    id: 'game_priority',
    scope: 'session',
    risk: 'low',
    evidence: 'version_dependent',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'IFEO\\<exe>\\PerfOptions  CpuPriorityClass = 3 (High) · IoPriority = 3',
    appliesTo: ['cpu_bound', 'mixed'],
    trueEffect: 0.4,
    persistentCandidate: false,
  },
  {
    id: 'background_isolation',
    scope: 'session',
    risk: 'medium',
    evidence: 'proven',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'EcoQoS + BelowNormal on idle background apps (never: audio, voice in call, launcher, anti-cheat)',
    appliesTo: ['cpu_bound', 'memory_bound', 'mixed'],
    trueEffect: 7,
    persistentCandidate: false,
  },
  {
    id: 'standby_purge',
    scope: 'session',
    risk: 'medium',
    evidence: 'disputed',
    requiresReboot: false,
    anticheatSafe: true,
    technical: 'NtSetSystemInformation(MemoryPurgeStandbyList) when free RAM < 400 MB, max 1/60 s',
    appliesTo: ['memory_bound'],
    needs: 'low_ram',
    trueEffect: -3,
    persistentCandidate: false,
  },
  {
    id: 'timer_resolution',
    scope: 'session',
    risk: 'low',
    evidence: 'disputed',
    requiresReboot: false,
    anticheatSafe: false,
    technical: 'NtSetTimerResolution(5000) while the game window is focused (ignored when occluded on Win11)',
    appliesTo: 'all',
    trueEffect: 0.2,
    persistentCandidate: false,
  },
];

export const tweakById = (id: string): MockTweak | undefined => CATALOG.find((t) => t.id === id);
