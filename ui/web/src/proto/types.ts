// overdrive.v1 — protocol types shared by the UI and the (mock) service.
// Mirrors docs/architecture/interfaces.md (EngineSnapshot) and ipc-protocol.md.
// In Phase 3 these are generated from crates/overdrive-proto; keep names identical.

export type Locale = 'es' | 'en';
export type UxMode = 'easy' | 'expert';
export type Theme = 'auto' | 'dark' | 'light';
export type Confidence = 'exact' | 'inferred' | 'unknown';
export type Risk = 'low' | 'medium' | 'high';
export type Evidence = 'proven' | 'version_dependent' | 'disputed';
export type Scope = 'session' | 'persistent';
export type Tier = 1 | 2 | 3;
export type AntiCheat = 'none' | 'kernel' | 'user_mode';
export type Bottleneck =
  | 'gpu_bound'
  | 'cpu_bound'
  | 'memory_bound'
  | 'io_bound'
  | 'thermal_or_power_limited'
  | 'mixed'
  | 'unknown';

export interface Field<T> {
  value: T | null;
  confidence: Confidence;
}

export interface HardwareSummary {
  tier: Tier;
  isLaptop: boolean;
  powerSource: 'ac' | 'battery';
  os: string;
  cpu: Field<string>;
  gpu: Field<string>;
  gpuSecondary: Field<string> | null;
  vramGb: Field<number>;
  ramGb: Field<number>;
  ramSpeed: Field<{ configured: number; rated: number }>;
  ramChannels: Field<number>;
  storage: Field<string>;
  storageFreePct: Field<number>;
  boardVendor: Field<string>;
}

export type FixPath =
  | { kind: 'software'; tweakId: string }
  | { kind: 'bios'; flowId: string }
  | { kind: 'hardware' }
  | { kind: 'user' };

export interface StructuralIssue {
  id: string;
  impact: 'high' | 'medium' | 'low';
  confidence: Confidence;
  fixableBy: FixPath;
  params?: Record<string, string | number>;
}

export type PulseState =
  | { kind: 'idle' }
  | { kind: 'bottleneck'; label: Bottleneck; intensity: number }
  | { kind: 'thermal'; pressure: number }
  | { kind: 'session'; label: Bottleneck };

export type GameStatus = 'needs_baseline' | 'ready' | 'armed' | 'running' | 'optimized';

export interface Measured {
  low1DeltaPct: number;
  ciLow: number;
  ciHigh: number;
  runs: number;
}

export interface GameCard {
  id: string;
  name: string;
  launcher: 'steam' | 'epic' | 'riot' | 'ea' | 'xbox' | 'other';
  antiCheat: AntiCheat;
  antiCheatName: string | null;
  bottleneck: Bottleneck;
  bottleneckParams?: Record<string, string | number>;
  status: GameStatus;
  fixesReady: number;
  measured: Measured | null;
  hue: number; // decorative art hue for the card
}

export interface SessionView {
  gameId: string;
  startedAt: number;
  fps: number;
  low1: number;
  frametimeMs: number;
  gpuBusyMs: number;
  recent: number[]; // last N frametimes (ms)
  bottleneck: Bottleneck;
  tweaksActive: number;
}

export type OpState = 'queued' | 'running' | 'cancelling';

export interface OperationView {
  opId: string;
  method: Method;
  labelKey: string;
  labelParams?: Record<string, string | number>;
  step: number;
  total: number;
  stepKey: string;
  stepParams?: Record<string, string | number>;
  startedAt: number;
  deadlineMs: number;
  cancellable: boolean;
  blocking: boolean; // true = UI shows a modal progress (only while present in snapshot)
  state: OpState;
}

export interface PersistentChangeView {
  tweakId: string;
  since: number;
  source: 'user_accept' | 'drift_repair' | 'bios_coach';
}

export interface SuspendedView {
  image: string;
  action: 'lower_priority' | 'eco_qos' | 'suspend';
}

export type Prompt =
  | { promptId: string; kind: 'keep_persistent'; tweakId: string; gameId: string; runs: number; deltaPct: number }
  | { promptId: string; kind: 'recovered' }
  | { promptId: string; kind: 'drift'; tweakId: string; cause: 'windows_update' | 'driver_update'; repaired: boolean }
  | { promptId: string; kind: 'bios_verified'; flowId: string; before: number; after: number }
  | { promptId: string; kind: 'notice'; messageKey: string; params?: Record<string, string | number> };

export interface ReportSummary {
  txId: number;
  gameId: string;
  endedAt: number;
  low1DeltaPct: number;
}

export interface HealthSummary {
  month: string; // YYYY-MM
  keptFixed: number;
  driftRepaired: number;
  sessionsMeasured: number;
  recommendationKey: string;
}

export interface CoachStep {
  kind: 'explain' | 'instruction' | 'reboot';
  key: string;
  params?: Record<string, string | number>;
}

export interface CoachState {
  flowId: string;
  state: 'started' | 'verified_applied' | 'not_applied' | 'watching' | 'done';
  steps: CoachStep[];
  watchUntil: number | null;
}

export interface Settings {
  mode: UxMode;
  locale: Locale;
  theme: Theme;
  reduceMotion: 'auto' | 'on' | 'off';
  telemetryOptIn: boolean;
}

export interface EngineSnapshot {
  seq: number;
  bootId: string;
  settings: Settings;
  hardware: HardwareSummary | null;
  issues: StructuralIssue[];
  pulse: PulseState;
  games: GameCard[];
  activeSession: SessionView | null;
  operations: OperationView[];
  persistentChanges: PersistentChangeView[];
  suspended: SuspendedView[];
  prompts: Prompt[];
  reports: ReportSummary[];
  health: HealthSummary | null;
  coach: CoachState | null;
}

// ---- Plan preview (Layer 2) ----
export type PlanLock = 'anticheat' | 'easy_mode' | 'blacklisted' | null;
export type TweakHistory = 'untested' | 'kept' | 'no_benefit' | 'harmful';

export interface PlanItem {
  tweakId: string;
  scope: Scope;
  risk: Risk;
  evidence: Evidence;
  requiresReboot: boolean;
  enabled: boolean;
  lock: PlanLock;
  history: TweakHistory;
  technical: string; // exact mechanism, shown in Expert mode
  before: string; // i18n key
  after: string; // i18n key
}

export interface PlanPreview {
  gameId: string;
  items: PlanItem[];
  hiddenInEasy: number;
}

// ---- Layer 3 ----
export interface Metrics {
  avgFps: number;
  low1: number;
  low01: number;
  p50Ms: number;
  p99Ms: number;
  stdevMs: number;
  stutters: number;
}

export type Verdict = 'kept' | 'no_benefit' | 'harmful' | 'inconclusive';

export interface SessionReport {
  txId: number;
  gameId: string;
  endedAt: number;
  gameplaySecs: number;
  bottleneck: Bottleneck;
  baselineRuns: number;
  baseline: Metrics;
  optimized: Metrics;
  baselineSeries: number[]; // frametime ms, downsampled
  optimizedSeries: number[];
  tweaks: { tweakId: string; verdict: Verdict; deltaPct: number | null }[];
}

// ---- Wire messages ----
export type Method =
  | 'hello'
  | 'state.snapshot'
  | 'hardware.rescan'
  | 'plan.preview'
  | 'plan.setToggle'
  | 'session.optimize'
  | 'session.benchmark'
  | 'persistent.accept'
  | 'persistent.revert'
  | 'tx.revertAll'
  | 'background.resumeAll'
  | 'bios.startFlow'
  | 'bios.rebootToFirmware'
  | 'report.session'
  | 'report.health'
  | 'prompt.answer'
  | 'settings.set'
  | 'op.cancel';

export interface ErrorBody {
  code: string;
  messageKey: string;
  details?: Record<string, string | number>;
}

export type Req = { kind: 'req'; id: number; method: Method; params?: unknown };
export type Res =
  | { kind: 'res'; id: number; ok: true; result: unknown }
  | { kind: 'res'; id: number; ok: false; error: ErrorBody };

export type OpOutcome = 'done' | 'failed' | 'timed_out' | 'cancelled';

export type Evt =
  | { kind: 'evt'; seq: number; type: 'state.patch'; data: Partial<EngineSnapshot> }
  | { kind: 'evt'; seq: number; type: 'op.progress'; data: { opId: string; step: number; total: number; elapsedMs: number } }
  | { kind: 'evt'; seq: number; type: 'op.finished'; data: { opId: string; method: Method; outcome: OpOutcome; error?: ErrorBody } }
  | { kind: 'evt'; seq: number; type: 'session.report'; data: { txId: number } };

export type Frame = Req | Res | Evt;

export interface HelloResult {
  bootId: string;
  svcVersion: string;
  protocol: 1;
}
