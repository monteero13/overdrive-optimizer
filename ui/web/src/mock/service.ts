// MockService — an in-page stand-in for overdrive-svc that speaks overdrive.v1.
// Everything the UI can show is reachable from here, including the bad paths:
// slow operations (deadline → timed_out), failing steps (→ revert + failed),
// cancellation, service crash + recovery, seq gaps, drift, BIOS reboot.
// Dev-only controls live in `MockControls` and are NOT part of the protocol.

import type { ServerEndpoint } from '../engine/transport.ts';
import type {
  EngineSnapshot,
  ErrorBody,
  GameCard,
  Method,
  Metrics,
  OperationView,
  PlanItem,
  PlanPreview,
  Prompt,
  Req,
  SessionReport,
  Settings,
  TweakHistory,
  Verdict,
} from '../proto/types.ts';
import { CATALOG, tweakById, type MockTweak } from './catalog.ts';
import { PRESETS, gamesFor, type Preset, type PresetId } from './presets.ts';

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const rid = (p: string) => `${p}_${Math.random().toString(36).slice(2, 8)}`;
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

const GAME_FACTOR: Record<string, number> = {
  'epic:fortnite': 1,
  'riot:valorant': 1.9,
  'steam:730': 1.35,
  'steam:1245620': 0.72,
  'xbox:minecraft': 1.5,
  'steam:367520': 2.4,
};

class Fail extends Error {
  body: ErrorBody;
  constructor(body: ErrorBody) {
    super(body.code);
    this.body = body;
  }
}

interface OpStep {
  key: string;
  params?: Record<string, string | number>;
  ms: number;
}

interface OpDef {
  method: Method;
  labelKey: string;
  labelParams?: Record<string, string | number>;
  steps: OpStep[];
  blocking: boolean;
  cancellable: boolean;
  deadlineMs: number;
  onDone: () => void;
}

export interface Scenario {
  slow: boolean;
  failNext: boolean;
}

export interface LinkHooks {
  down(): void;
  up(): void;
}

export class MockService implements ServerEndpoint {
  state!: EngineSnapshot;
  scenario: Scenario = { slow: false, failNext: false };
  preset!: Preset;
  link: LinkHooks | null = null;

  private push: ((f: string) => void) | null = null;
  private skipSeq = false;
  private toggles = new Map<string, Map<string, boolean>>(); // gameId -> tweakId -> enabled
  private history = new Map<string, { verdict: Verdict; runs: number; deltaPct: number }>(); // `${tweak}|${game}`
  private reports = new Map<number, SessionReport>();
  private txSeq = 100;
  private cancelRequests = new Set<string>();
  private sessionTimer: ReturnType<typeof setInterval> | null = null;
  private sessionTweaks: string[] = [];
  private sessionPrevStatus: GameCard['status'] = 'ready';

  constructor(presetId: PresetId = 'laptop_hybrid') {
    this.reset(presetId);
  }

  // ------------------------------------------------------------ lifecycle
  reset(presetId: PresetId): void {
    this.stopSessionTimer();
    this.preset = PRESETS[presetId];
    this.toggles.clear();
    this.history.clear();
    this.reports.clear();
    const settings: Settings = this.state?.settings ?? {
      mode: 'easy',
      locale: 'es',
      theme: 'auto',
      reduceMotion: 'auto',
      telemetryOptIn: false,
    };
    this.state = {
      seq: 0,
      bootId: rid('boot'),
      settings,
      hardware: clone(this.preset.hardware),
      issues: clone(this.preset.issues),
      pulse: this.idlePulse(),
      games: gamesFor(this.preset),
      activeSession: null,
      operations: [],
      persistentChanges: [],
      suspended: [],
      prompts: [],
      reports: [],
      health: {
        month: new Date().toISOString().slice(0, 7),
        keptFixed: 0,
        driftRepaired: 0,
        sessionsMeasured: 0,
        recommendationKey: this.preset.issues[0] ? `health.rec.${this.preset.issues[0].id}` : 'health.rec.none',
      },
      coach: null,
    };
    // Seed: one game already measured in an earlier session, so Layer 3 has history.
    this.seedHistory();
    this.refreshFixCounts();
  }

  /** Simulated service restart (crash or reboot). Journal recovery runs before IPC is accepted. */
  private async restart(downMs: number, after: () => void): Promise<void> {
    this.link?.down();
    this.stopSessionTimer();
    await sleep(downMs);
    this.state.bootId = rid('boot');
    this.state.seq = 0;
    this.state.operations = [];
    after();
    this.link?.up();
  }

  attach(push: (f: string) => void): void {
    this.push = push;
  }

  detach(): void {
    this.push = null;
  }

  // ------------------------------------------------------------ wire
  receive(frame: string, reply: (f: string) => void): void {
    let req: Req;
    try {
      req = JSON.parse(frame) as Req;
    } catch {
      return;
    }
    try {
      const result = this.dispatch(req.method, (req.params ?? {}) as Record<string, unknown>);
      reply(JSON.stringify({ kind: 'res', id: req.id, ok: true, result: result ?? null }));
    } catch (e) {
      const error: ErrorBody = e instanceof Fail ? e.body : { code: 'INTERNAL', messageKey: 'err.internal' };
      reply(JSON.stringify({ kind: 'res', id: req.id, ok: false, error }));
    }
  }

  private nextSeq(): number {
    this.state.seq += this.skipSeq ? 2 : 1;
    this.skipSeq = false;
    return this.state.seq;
  }

  private commit(...keys: (keyof EngineSnapshot)[]): void {
    const seq = this.nextSeq();
    const data: Record<string, unknown> = {};
    for (const k of keys) data[k] = this.state[k];
    this.push?.(JSON.stringify({ kind: 'evt', seq, type: 'state.patch', data }));
  }

  private event(type: 'op.progress' | 'op.finished' | 'session.report', data: unknown): void {
    const seq = this.nextSeq();
    this.push?.(JSON.stringify({ kind: 'evt', seq, type, data }));
  }

  private dispatch(method: Method, p: Record<string, unknown>): unknown {
    switch (method) {
      case 'hello':
        return { bootId: this.state.bootId, svcVersion: '1.0.0-mock', protocol: 1 };
      case 'state.snapshot':
        return clone(this.state);
      case 'hardware.rescan':
        return { opId: this.runOp({ method, labelKey: 'op.rescan', steps: [{ key: 'step.rescan_hw', ms: 900 }, { key: 'step.rescan_issues', ms: 600 }], blocking: false, cancellable: false, deadlineMs: 10_000, onDone: () => {} }) };
      case 'plan.preview':
        return this.plan(this.gameId(p));
      case 'plan.setToggle': {
        const gameId = this.gameId(p);
        const item = this.plan(gameId).items.find((i) => i.tweakId === p.tweakId);
        if (!item) throw new Fail({ code: 'UNKNOWN_TWEAK', messageKey: 'err.unknown_tweak' });
        if (item.lock) throw new Fail({ code: 'TWEAK_LOCKED', messageKey: `lock.${item.lock}` });
        this.gameToggles(gameId).set(item.tweakId, Boolean(p.enabled));
        this.refreshFixCounts();
        this.commit('games');
        return this.plan(gameId);
      }
      case 'session.optimize':
        return { opId: this.optimize(this.gameId(p)) };
      case 'session.benchmark':
        return { opId: this.benchmark(this.gameId(p)) };
      case 'persistent.accept':
        return { opId: this.acceptPersistent(String(p.tweakId)) };
      case 'persistent.revert':
        return { opId: this.revertPersistent(String(p.tweakId)) };
      case 'tx.revertAll':
        return { opId: this.revertAll() };
      case 'background.resumeAll':
        this.state.suspended = [];
        this.commit('suspended');
        return { ok: true };
      case 'bios.startFlow':
        return this.startCoach(String(p.issueId));
      case 'bios.rebootToFirmware':
        if (p.confirm !== true) throw new Fail({ code: 'CONFIRM_REQUIRED', messageKey: 'err.confirm_required' });
        return { opId: this.rebootToFirmware() };
      case 'report.session': {
        const r = this.reports.get(Number(p.txId));
        if (!r) throw new Fail({ code: 'NOT_FOUND', messageKey: 'err.not_found' });
        return r;
      }
      case 'report.health':
        return this.state.health;
      case 'prompt.answer':
        this.answer(String(p.promptId), String(p.choice));
        return { ok: true };
      case 'settings.set': {
        this.state.settings = { ...this.state.settings, ...(p as Partial<Settings>) };
        this.refreshFixCounts();
        this.commit('settings', 'games');
        return this.state.settings;
      }
      case 'op.cancel': {
        const op = this.state.operations.find((o) => o.opId === p.opId);
        if (!op) throw new Fail({ code: 'NOT_FOUND', messageKey: 'err.not_found' });
        if (!op.cancellable) throw new Fail({ code: 'NOT_CANCELLABLE', messageKey: 'err.not_cancellable' });
        this.cancelRequests.add(op.opId);
        return { ok: true };
      }
    }
  }

  private gameId(p: Record<string, unknown>): string {
    const id = String(p.gameId ?? '');
    if (!this.state.games.some((g) => g.id === id)) throw new Fail({ code: 'UNKNOWN_GAME', messageKey: 'err.unknown_game' });
    return id;
  }

  private game(id: string): GameCard {
    return this.state.games.find((g) => g.id === id)!;
  }

  // ------------------------------------------------------------ operations
  private runOp(def: OpDef): string {
    const op: OperationView = {
      opId: rid('op'),
      method: def.method,
      labelKey: def.labelKey,
      labelParams: def.labelParams,
      step: 0,
      total: def.steps.length,
      stepKey: def.steps[0]?.key ?? 'step.working',
      stepParams: def.steps[0]?.params,
      startedAt: Date.now(),
      deadlineMs: def.deadlineMs,
      cancellable: def.cancellable,
      blocking: def.blocking,
      state: 'running',
    };
    const fail = this.scenario.failNext;
    this.scenario.failNext = false;
    this.state.operations = [...this.state.operations, op];
    this.commit('operations');

    void (async () => {
      const slow = this.scenario.slow ? 6 : 1;
      const failAt = fail ? Math.min(2, def.steps.length - 1) : -1;
      for (let i = 0; i < def.steps.length; i++) {
        const s = def.steps[i];
        this.patchOp(op.opId, { step: i + 1, stepKey: s.key, stepParams: s.params });
        this.event('op.progress', { opId: op.opId, step: i + 1, total: def.steps.length, elapsedMs: Date.now() - op.startedAt });
        await sleep(s.ms * slow);
        if (!this.hasOp(op.opId)) return; // service restarted underneath
        if (this.cancelRequests.delete(op.opId)) {
          this.patchOp(op.opId, { state: 'cancelling', stepKey: 'step.reverting' });
          await sleep(700);
          return this.finishOp(op, 'cancelled');
        }
        if (Date.now() - op.startedAt > def.deadlineMs) {
          this.patchOp(op.opId, { stepKey: 'step.reverting' });
          await sleep(700);
          return this.finishOp(op, 'timed_out', { code: 'DEADLINE', messageKey: 'err.timed_out' });
        }
        if (i === failAt) {
          this.patchOp(op.opId, { stepKey: 'step.reverting' });
          await sleep(800);
          return this.finishOp(op, 'failed', { code: 'STEP_FAILED', messageKey: 'err.step_failed', details: { step: s.key } });
        }
      }
      def.onDone();
      this.finishOp(op, 'done');
    })();
    return op.opId;
  }

  private hasOp(id: string): boolean {
    return this.state.operations.some((o) => o.opId === id);
  }

  private patchOp(id: string, patch: Partial<OperationView>): void {
    this.state.operations = this.state.operations.map((o) => (o.opId === id ? { ...o, ...patch } : o));
    this.commit('operations');
  }

  private finishOp(op: OperationView, outcome: 'done' | 'failed' | 'timed_out' | 'cancelled', error?: ErrorBody): void {
    this.state.operations = this.state.operations.filter((o) => o.opId !== op.opId);
    this.commit('operations');
    this.event('op.finished', { opId: op.opId, method: op.method, outcome, error });
  }

  // ------------------------------------------------------------ plan
  private gameToggles(gameId: string): Map<string, boolean> {
    let m = this.toggles.get(gameId);
    if (!m) this.toggles.set(gameId, (m = new Map()));
    return m;
  }

  private applicable(t: MockTweak, g: GameCard): boolean {
    if (t.appliesTo !== 'all' && !t.appliesTo.includes(g.bottleneck)) return false;
    if (t.needs === 'hybrid_gpu' && !this.preset.hybridGpu) return false;
    if (t.needs === 'nvidia' && !this.preset.nvidia) return false;
    if (t.needs === 'low_ram' && !this.preset.lowRam) return false;
    return true;
  }

  private plan(gameId: string): PlanPreview {
    const g = this.game(gameId);
    const easy = this.state.settings.mode === 'easy';
    const items: PlanItem[] = [];
    let hidden = 0;
    for (const t of CATALOG) {
      if (!this.applicable(t, g)) continue;
      const h = this.history.get(`${t.id}|${gameId}`);
      const history: TweakHistory = h ? (h.verdict === 'inconclusive' ? 'untested' : h.verdict) : 'untested';
      // Easy mode never shows anything but low-risk, proven changes — whatever else applies.
      if (easy && !(t.risk === 'low' && t.evidence === 'proven')) {
        hidden++;
        continue;
      }
      let lock: PlanItem['lock'] = null;
      if (history === 'harmful') lock = 'blacklisted';
      else if (g.antiCheat === 'kernel' && !t.anticheatSafe) lock = 'anticheat';
      const persistentNow = this.state.persistentChanges.some((c) => c.tweakId === t.id);
      const def = !lock && t.evidence !== 'disputed' && history !== 'no_benefit' && !persistentNow;
      const enabled = lock ? false : (this.gameToggles(gameId).get(t.id) ?? def);
      items.push({
        tweakId: t.id,
        scope: t.scope,
        risk: t.risk,
        evidence: t.evidence,
        requiresReboot: t.requiresReboot,
        enabled,
        lock,
        history: persistentNow ? 'kept' : history,
        technical: t.technical,
        before: `tw.${t.id}.before`,
        after: `tw.${t.id}.after`,
      });
    }
    return { gameId, items, hiddenInEasy: hidden };
  }

  private enabledTweaks(gameId: string): string[] {
    return this.plan(gameId).items.filter((i) => i.enabled).map((i) => i.tweakId);
  }

  private refreshFixCounts(): void {
    this.state.games = this.state.games.map((g) => ({ ...g, fixesReady: this.enabledTweaks(g.id).length }));
  }

  // ------------------------------------------------------------ flows
  private optimize(gameId: string): string {
    const g = this.game(gameId);
    if (g.status === 'needs_baseline') throw new Fail({ code: 'NEEDS_BASELINE', messageKey: 'err.needs_baseline' });
    const n = this.enabledTweaks(gameId).length;
    if (n === 0) throw new Fail({ code: 'NOTHING_TO_APPLY', messageKey: 'err.nothing_to_apply' });
    return this.runOp({
      method: 'session.optimize',
      labelKey: 'op.optimize',
      labelParams: { game: g.name },
      steps: [
        { key: 'step.check_anticheat', params: { ac: g.antiCheatName ?? '—' }, ms: 600 },
        { key: 'step.journal', ms: 700 },
        { key: 'step.arm', params: { n }, ms: 700 },
        { key: 'step.verify', ms: 500 },
      ],
      blocking: true,
      cancellable: true,
      deadlineMs: 15_000,
      onDone: () => {
        this.setGame(gameId, { status: this.state.activeSession?.gameId === gameId ? 'running' : 'armed' });
        if (this.state.activeSession?.gameId === gameId) {
          this.sessionTweaks = this.enabledTweaks(gameId);
          this.state.activeSession = { ...this.state.activeSession, tweaksActive: this.sessionTweaks.length };
          this.commit('activeSession');
        }
      },
    });
  }

  private benchmark(gameId: string): string {
    const g = this.game(gameId);
    return this.runOp({
      method: 'session.benchmark',
      labelKey: 'op.benchmark',
      labelParams: { game: g.name },
      steps: [
        { key: 'step.bench_launch', ms: 900 },
        { key: 'step.bench_run', params: { run: 1, of: 3 }, ms: 1200 },
        { key: 'step.bench_run', params: { run: 2, of: 3 }, ms: 1200 },
        { key: 'step.bench_run', params: { run: 3, of: 3 }, ms: 1200 },
        { key: 'step.bench_noise', ms: 600 },
      ],
      blocking: true,
      cancellable: true,
      deadlineMs: 30_000,
      onDone: () => {
        this.setGame(gameId, { status: 'ready' });
        this.pushPrompt({ promptId: rid('p'), kind: 'notice', messageKey: 'notice.baseline_saved', params: { game: g.name } });
      },
    });
  }

  private acceptPersistent(tweakId: string): string {
    if (!tweakById(tweakId)) throw new Fail({ code: 'UNKNOWN_TWEAK', messageKey: 'err.unknown_tweak' });
    return this.runOp({
      method: 'persistent.accept',
      labelKey: 'op.persist',
      labelParams: { tweak: tweakId },
      steps: [
        { key: 'step.restore_point', ms: 900 },
        { key: 'step.journal', ms: 500 },
        { key: 'step.apply', ms: 500 },
        { key: 'step.verify', ms: 400 },
      ],
      blocking: false,
      cancellable: false,
      deadlineMs: 20_000,
      onDone: () => {
        if (!this.state.persistentChanges.some((c) => c.tweakId === tweakId))
          this.state.persistentChanges = [...this.state.persistentChanges, { tweakId, since: Date.now(), source: 'user_accept' }];
        if (this.state.health) this.state.health = { ...this.state.health, keptFixed: this.state.persistentChanges.length };
        this.refreshFixCounts();
        this.commit('persistentChanges', 'health', 'games');
      },
    });
  }

  private revertPersistent(tweakId: string): string {
    return this.runOp({
      method: 'persistent.revert',
      labelKey: 'op.revert_one',
      labelParams: { tweak: tweakId },
      steps: [
        { key: 'step.revert', ms: 600 },
        { key: 'step.verify_revert', ms: 500 },
      ],
      blocking: false,
      cancellable: false,
      deadlineMs: 15_000,
      onDone: () => {
        this.state.persistentChanges = this.state.persistentChanges.filter((c) => c.tweakId !== tweakId);
        if (this.state.health) this.state.health = { ...this.state.health, keptFixed: this.state.persistentChanges.length };
        this.refreshFixCounts();
        this.commit('persistentChanges', 'health', 'games');
      },
    });
  }

  private revertAll(): string {
    const steps: OpStep[] = this.state.persistentChanges.map((c) => ({ key: 'step.revert_named', params: { tweak: c.tweakId }, ms: 500 }));
    steps.push({ key: 'step.resume_background', ms: 400 }, { key: 'step.diff_journal', ms: 900 });
    return this.runOp({
      method: 'tx.revertAll',
      labelKey: 'op.revert_all',
      steps,
      blocking: true,
      cancellable: false,
      deadlineMs: 60_000,
      onDone: () => {
        this.state.persistentChanges = [];
        this.state.suspended = [];
        this.state.games = this.state.games.map((g) => (g.status === 'armed' ? { ...g, status: 'ready' } : g));
        if (this.state.health) this.state.health = { ...this.state.health, keptFixed: 0 };
        this.refreshFixCounts();
        this.pushPrompt({ promptId: rid('p'), kind: 'notice', messageKey: 'notice.reset_done' });
        this.commit('persistentChanges', 'suspended', 'games', 'health');
      },
    });
  }

  private startCoach(issueId: string) {
    const issue = this.state.issues.find((i) => i.id === issueId);
    if (!issue || issue.fixableBy.kind !== 'bios') throw new Fail({ code: 'NOT_FOUND', messageKey: 'err.not_found' });
    // Same keys and params as overdrive-core::coach::flow_for (Rust).
    const vendor = (this.preset.hardware.boardVendor.value ?? 'generic').toLowerCase();
    const menuVendor = ['msi', 'asus', 'gigabyte', 'asrock'].includes(vendor) ? vendor : 'generic';
    const flowId = issue.fixableBy.flowId;
    const p = issue.params ?? {};
    const ram = flowId === 'ram_xmp';
    const target = ram ? Number(p.rated) : (this.preset.hardware.ramGb.value ?? 8) >= 15 ? 2048 : 1024;
    const steps = [2133, 2400, 2666, 2933, 3000, 3200, 3466, 3600];
    const fallback = ram ? [...steps].reverse().find((s) => s < target && s > Number(p.configured)) : undefined;
    const profile = vendor === 'asus' ? 'DOCP / EXPO' : vendor === 'msi' ? 'A-XMP / EXPO' : 'XMP / EXPO';
    this.state.coach = {
      flowId,
      state: 'started',
      watchUntil: null,
      steps: [
        { kind: 'explain', key: `coach.${flowId}.explain`, params: { ...p, target } },
        { kind: 'reboot', key: 'coach.reboot' },
        { kind: 'instruction', key: `coach.vendor.${vendor}.enter` },
        { kind: 'instruction', key: `coach.${flowId}.where.${menuVendor}`, params: { profile } },
        { kind: 'instruction', key: `coach.${flowId}.set`, params: { profile, target, ...(fallback ? { fallback } : {}) } },
        { kind: 'instruction', key: 'coach.save_exit' },
      ],
    };
    this.commit('coach');
    return this.state.coach;
  }

  private rebootToFirmware(): string {
    const flowId = this.state.coach?.flowId ?? 'ram_xmp';
    return this.runOp({
      method: 'bios.rebootToFirmware',
      labelKey: 'op.reboot_fw',
      steps: [
        { key: 'step.save_state', ms: 700 },
        { key: 'step.reboot_fw', ms: 900 },
      ],
      blocking: true,
      cancellable: false,
      deadlineMs: 20_000,
      onDone: () => {
        void this.restart(2600, () => {
          // After reboot: profiler re-reads SMBIOS, coach verifies the change applied.
          const issue = this.state.issues.find((i) => i.fixableBy.kind === 'bios' && i.fixableBy.flowId === flowId);
          let before = 0;
          let after = 0;
          if (flowId === 'ram_xmp' && this.state.hardware?.ramSpeed.value) {
            before = this.state.hardware.ramSpeed.value.configured;
            after = this.state.hardware.ramSpeed.value.rated;
            this.state.hardware.ramSpeed.value = { configured: after, rated: after };
          } else if (flowId === 'uma_buffer' && this.state.hardware) {
            before = 512;
            after = 2048;
            this.state.hardware.vramGb.value = 2;
          }
          if (issue) this.state.issues = this.state.issues.filter((i) => i !== issue);
          this.state.coach = this.state.coach
            ? { ...this.state.coach, state: 'watching', watchUntil: Date.now() + 3 * 86_400_000 }
            : null;
          this.state.persistentChanges = [...this.state.persistentChanges, { tweakId: `bios_${flowId}`, since: Date.now(), source: 'bios_coach' }];
          this.state.prompts = [...this.state.prompts, { promptId: rid('p'), kind: 'bios_verified', flowId, before, after }];
          this.preset = { ...this.preset, baseFps: Math.round(this.preset.baseFps * 1.12) };
        });
      },
    });
  }

  private answer(promptId: string, choice: string): void {
    const p = this.state.prompts.find((x) => x.promptId === promptId);
    if (!p) throw new Fail({ code: 'NOT_FOUND', messageKey: 'err.not_found' });
    this.state.prompts = this.state.prompts.filter((x) => x.promptId !== promptId);
    this.commit('prompts');
    if (p.kind === 'keep_persistent' && choice === 'yes') this.acceptPersistent(p.tweakId);
    if (p.kind === 'bios_verified' && this.state.coach) {
      this.state.coach = { ...this.state.coach, state: 'done' };
      this.commit('coach');
    }
  }

  private pushPrompt(p: Prompt): void {
    this.state.prompts = [...this.state.prompts, p];
    this.commit('prompts');
  }

  private setGame(id: string, patch: Partial<GameCard>): void {
    this.state.games = this.state.games.map((g) => (g.id === id ? { ...g, ...patch } : g));
    this.commit('games');
  }

  private idlePulse(): EngineSnapshot['pulse'] {
    const top = this.preset.issues[0];
    if (!top) return { kind: 'idle' };
    const label = top.id === 'game_on_igpu' || top.id === 'uma_small' ? 'gpu_bound' : 'memory_bound';
    return { kind: 'bottleneck', label, intensity: top.impact === 'high' ? 0.8 : 0.45 };
  }

  // ------------------------------------------------------------ session simulation
  private effectOf(tweaks: string[], gameId: string): number {
    const g = this.game(gameId);
    let e = 0;
    for (const id of tweaks) {
      const t = tweakById(id);
      if (!t) continue;
      let v = t.trueEffect;
      if (id === 'hybrid_gpu' && g.bottleneck !== 'gpu_bound') v *= 0.3;
      e += v;
    }
    return Math.max(-20, Math.min(60, e)) / 100;
  }

  private baseFrametime(gameId: string): number {
    return 1000 / (this.preset.baseFps * (GAME_FACTOR[gameId] ?? 1));
  }

  private series(gameId: string, effect: number, n = 240): number[] {
    // Calibrated so the 1 % low gain ≈ the summed true effect (e.g. +20 % effect → ~+16–22 %),
    // and spikes stay frequent enough (> 1 %) that p99 still lands on them, like real games.
    const base = this.baseFrametime(gameId) / (1 + effect * 0.35);
    const spikeP = Math.max(0.014, 0.026 * (1 - effect));
    const spikeK = Math.max(1.5, 2.3 - effect * 0.9);
    // Long series (reports): a fixed number of spikes at random positions, so p99 always
    // lands on a spike and the 1 % low reflects the effect, not sampling luck.
    // Short chunks (live HUD): probabilistic spikes.
    const spikes = new Set<number>();
    if (n >= 100) {
      const count = Math.max(4, Math.round(spikeP * n));
      while (spikes.size < count) spikes.add(Math.floor(Math.random() * n));
    }
    const out: number[] = [];
    for (let i = 0; i < n; i++) {
      const wave = Math.sin(i / 17) * 0.06 + Math.sin(i / 5.3) * 0.03;
      let v = base * (1 + wave + (Math.random() - 0.5) * 0.08);
      if (n >= 100 ? spikes.has(i) : Math.random() < spikeP) v *= spikeK + Math.random() * 0.25;
      out.push(Math.round(v * 100) / 100);
    }
    return out;
  }

  private metrics(s: number[]): Metrics {
    const sorted = [...s].sort((a, b) => a - b);
    const q = (p: number) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
    const mean = s.reduce((a, b) => a + b, 0) / s.length;
    const median = q(0.5);
    const sd = Math.sqrt(s.reduce((a, b) => a + (b - mean) ** 2, 0) / s.length);
    const r = (v: number) => Math.round(v * 10) / 10;
    return {
      avgFps: r(1000 / mean),
      low1: r(1000 / q(0.99)),
      low01: r(1000 / q(0.999)),
      p50Ms: r(median),
      p99Ms: r(q(0.99)),
      stdevMs: Math.round(sd * 100) / 100,
      stutters: s.filter((v) => v > median * 2).length,
    };
  }

  launchGame(gameId: string): void {
    if (this.state.activeSession) return;
    const g = this.game(gameId);
    this.sessionPrevStatus = g.status;
    this.sessionTweaks = g.status === 'armed' ? this.enabledTweaks(gameId) : [];
    this.state.activeSession = {
      gameId,
      startedAt: Date.now(),
      fps: 0,
      low1: 0,
      frametimeMs: 0,
      gpuBusyMs: 0,
      recent: [],
      bottleneck: g.bottleneck,
      tweaksActive: 0,
    };
    this.state.pulse = { kind: 'session', label: g.bottleneck };
    this.setGame(gameId, { status: 'running' });
    this.commit('activeSession', 'pulse');
    if (this.sessionTweaks.length) {
      const n = this.sessionTweaks.length;
      this.runOp({
        method: 'session.optimize',
        labelKey: 'op.applying_session',
        labelParams: { game: g.name, n },
        steps: [
          { key: 'step.journal', ms: 400 },
          { key: 'step.apply_n', params: { n }, ms: 600 },
          { key: 'step.verify', ms: 300 },
        ],
        blocking: false,
        cancellable: false,
        deadlineMs: 10_000,
        onDone: () => {
          if (this.state.activeSession) {
            this.state.activeSession = { ...this.state.activeSession, tweaksActive: n };
            const bg = this.sessionTweaks.includes('background_isolation');
            if (bg) this.state.suspended = [
              { image: 'OneDrive.exe', action: 'eco_qos' },
              { image: 'msedge.exe', action: 'lower_priority' },
              { image: 'AdobeUpdateService.exe', action: 'suspend' },
            ];
            this.commit('activeSession', 'suspended');
          }
        },
      });
    }
    this.sessionTimer = setInterval(() => this.tick(), 500);
  }

  private tick(): void {
    const s = this.state.activeSession;
    if (!s) return;
    const effect = s.tweaksActive ? this.effectOf(this.sessionTweaks, s.gameId) : 0;
    const chunk = this.series(s.gameId, effect, 12);
    const recent = [...s.recent, ...chunk].slice(-120);
    const m = this.metrics(recent);
    const ft = chunk[chunk.length - 1];
    const gpuShare = s.bottleneck === 'gpu_bound' ? 0.96 : s.bottleneck === 'cpu_bound' ? 0.58 : 0.8;
    this.state.activeSession = {
      ...s,
      recent,
      fps: m.avgFps,
      low1: m.low1,
      frametimeMs: ft,
      gpuBusyMs: Math.round(ft * gpuShare * 10) / 10,
    };
    this.commit('activeSession');
  }

  closeGame(): void {
    const s = this.state.activeSession;
    if (!s) return;
    this.stopSessionTimer();
    const g = this.game(s.gameId);
    const tweaks = s.tweaksActive ? this.sessionTweaks : [];
    this.state.activeSession = null;
    this.state.pulse = this.idlePulse();
    this.commit('activeSession', 'pulse');

    if (!tweaks.length) {
      // Baseline run (or nothing armed): just learn.
      this.setGame(g.id, { status: this.sessionPrevStatus === 'needs_baseline' ? 'ready' : this.sessionPrevStatus === 'running' ? 'ready' : this.sessionPrevStatus });
      this.pushPrompt({ promptId: rid('p'), kind: 'notice', messageKey: 'notice.baseline_saved', params: { game: g.name } });
      return;
    }

    this.runOp({
      method: 'session.optimize',
      labelKey: 'op.reverting_session',
      labelParams: { game: g.name },
      steps: [
        { key: 'step.revert', ms: 500 },
        { key: 'step.resume_background', ms: 400 },
        { key: 'step.verify_revert', ms: 400 },
        { key: 'step.ab_verdict', ms: 700 },
      ],
      blocking: false,
      cancellable: false,
      deadlineMs: 15_000,
      onDone: () => {
        this.state.suspended = [];
        const report = this.buildReport(g, tweaks);
        this.reports.set(report.txId, report);
        const delta = Math.round((report.optimized.low1 / report.baseline.low1 - 1) * 1000) / 10;
        const prev = g.measured?.runs ?? 0;
        const runs = prev + 1;
        this.setGame(g.id, {
          status: 'optimized',
          measured: { low1DeltaPct: delta, ciLow: Math.round((delta - 9 / runs) * 10) / 10, ciHigh: Math.round((delta + 9 / runs) * 10) / 10, runs },
        });
        this.state.reports = [{ txId: report.txId, gameId: g.id, endedAt: report.endedAt, low1DeltaPct: delta }, ...this.state.reports];
        if (this.state.health) this.state.health = { ...this.state.health, sessionsMeasured: this.state.health.sessionsMeasured + 1 };
        this.refreshFixCounts();
        this.commit('suspended', 'reports', 'health', 'games');
        this.event('session.report', { txId: report.txId });
        const keep = report.tweaks.find(
          (t) => t.verdict === 'kept' && tweakById(t.tweakId)?.persistentCandidate && !this.state.persistentChanges.some((c) => c.tweakId === t.tweakId),
        );
        if (keep) this.pushPrompt({ promptId: rid('p'), kind: 'keep_persistent', tweakId: keep.tweakId, gameId: g.id, runs: Math.max(3, runs), deltaPct: keep.deltaPct ?? 0 });
      },
    });
  }

  private buildReport(g: GameCard, tweaks: string[]): SessionReport {
    const baselineSeries = this.series(g.id, 0);
    const optimizedSeries = this.series(g.id, this.effectOf(tweaks, g.id));
    const verdicts = tweaks.map((id) => {
      const t = tweakById(id)!;
      let v = t.trueEffect;
      if (id === 'hybrid_gpu' && g.bottleneck !== 'gpu_bound') v *= 0.3;
      const noise = (Math.random() - 0.5) * 1.2;
      const d = Math.round((v + noise) * 10) / 10;
      const verdict: Verdict = d >= 3 ? 'kept' : d <= -1.5 ? 'harmful' : Math.abs(d) <= 1.5 ? 'no_benefit' : 'inconclusive';
      this.history.set(`${id}|${g.id}`, { verdict, runs: 3, deltaPct: d });
      return { tweakId: id, verdict, deltaPct: verdict === 'no_benefit' ? null : d };
    });
    return {
      txId: ++this.txSeq,
      gameId: g.id,
      endedAt: Date.now(),
      gameplaySecs: Math.round((Date.now() - (this.state.activeSession?.startedAt ?? Date.now() - 900_000)) / 1000) || 900,
      bottleneck: g.bottleneck,
      baselineRuns: 3,
      baseline: this.metrics(baselineSeries),
      optimized: this.metrics(optimizedSeries),
      baselineSeries,
      optimizedSeries,
      tweaks: verdicts,
    };
  }

  private seedHistory(): void {
    const g = this.state.games.find((x) => x.id === 'steam:730')!;
    const tweaks = this.enabledTweaks(g.id).concat(['standby_purge']).filter((v, i, a) => a.indexOf(v) === i && this.applicable(tweakById(v)!, g));
    const r = this.buildReport(g, tweaks.length ? tweaks : ['gamedvr_off']);
    r.endedAt = Date.now() - 2 * 86_400_000;
    this.reports.set(r.txId, r);
    const delta = Math.round((r.optimized.low1 / r.baseline.low1 - 1) * 1000) / 10;
    this.state.reports = [{ txId: r.txId, gameId: g.id, endedAt: r.endedAt, low1DeltaPct: delta }];
    this.state.games = this.state.games.map((x) =>
      x.id === g.id ? { ...x, status: 'optimized', measured: { low1DeltaPct: delta, ciLow: Math.round((delta - 4) * 10) / 10, ciHigh: Math.round((delta + 4) * 10) / 10, runs: 3 } } : x,
    );
    if (this.state.health) this.state.health.sessionsMeasured = 3;
  }

  private stopSessionTimer(): void {
    if (this.sessionTimer) clearInterval(this.sessionTimer);
    this.sessionTimer = null;
  }

  // ------------------------------------------------------------ dev controls
  crash(): void {
    const s = this.state.activeSession;
    void this.restart(1800, () => {
      // recover(): the open session transaction is rolled back from the journal.
      if (s) {
        this.state.activeSession = null;
        this.state.pulse = this.idlePulse();
        this.state.suspended = [];
        this.state.games = this.state.games.map((g) => (g.id === s.gameId ? { ...g, status: s.tweaksActive ? 'armed' : 'ready' } : g));
      }
      this.state.prompts = [...this.state.prompts, { promptId: rid('p'), kind: 'recovered' }];
    });
  }

  drift(cause: 'windows_update' | 'driver_update'): void {
    const target = this.state.persistentChanges.find((c) => c.source !== 'bios_coach');
    if (!target) {
      this.pushPrompt({ promptId: rid('p'), kind: 'notice', messageKey: 'notice.drift_none' });
      return;
    }
    if (this.state.health) this.state.health = { ...this.state.health, driftRepaired: this.state.health.driftRepaired + 1 };
    this.commit('health');
    this.pushPrompt({ promptId: rid('p'), kind: 'drift', tweakId: target.tweakId, cause, repaired: true });
  }

  setThermal(on: boolean): void {
    if (this.state.activeSession) return;
    this.state.pulse = on ? { kind: 'thermal', pressure: 0.75 } : this.idlePulse();
    this.commit('pulse');
  }

  setBattery(on: boolean): void {
    if (!this.state.hardware) return;
    this.state.hardware = { ...this.state.hardware, powerSource: on ? 'battery' : 'ac' };
    this.state.issues = this.state.issues.filter((i) => i.id !== 'on_battery');
    if (on) this.state.issues = [{ id: 'on_battery', impact: 'high', confidence: 'exact', fixableBy: { kind: 'user' } }, ...this.state.issues];
    this.commit('hardware', 'issues');
  }

  gapNext(): void {
    this.skipSeq = true;
  }

  switchPreset(id: PresetId): void {
    void this.restart(900, () => {
      const settings = this.state.settings;
      this.reset(id);
      this.state.settings = settings;
    });
  }
}
