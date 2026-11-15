// Protocol + mock engine tests (Node ≥ 22, type stripping). Run: npm test
// Covers the Phase 2 DoD: every operation ends in a terminal state, no stuck
// operations after failures/timeouts/cancel/crash, seq gaps trigger resync,
// anti-cheat + Easy-mode locks, i18n parity and key coverage.
import { readFileSync } from 'node:fs';
import { EngineClient, type ClientEvent } from '../src/engine/client.ts';
import { applyMergePatch } from '../src/engine/mergePatch.ts';
import { MockTransport } from '../src/engine/transport.ts';
import { en } from '../src/i18n/en.ts';
import { es } from '../src/i18n/es.ts';
import { CATALOG } from '../src/mock/catalog.ts';
import { PRESETS } from '../src/mock/presets.ts';
import { MockService } from '../src/mock/service.ts';
import type { PlanPreview } from '../src/proto/types.ts';

let passed = 0;
let failed = 0;
const tests: [string, () => Promise<void> | void][] = [];
const test = (name: string, fn: () => Promise<void> | void) => tests.push([name, fn]);
function ok(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 8000, what = 'condition'): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error(`timeout waiting for ${what}`);
    await sleep(10);
  }
}

function setup(preset: 'laptop_hybrid' | 'desktop_entry' | 'apu_lowend' = 'laptop_hybrid') {
  const svc = new MockService(preset);
  const tr = new MockTransport(svc, 1);
  svc.link = { down: () => tr.disconnect(), up: () => tr.connect() };
  const client = new EngineClient(tr);
  const events: ClientEvent[] = [];
  client.on((e) => events.push(e));
  tr.connect();
  return { svc, tr, client, events };
}
const finished = (events: ClientEvent[], opId: string) =>
  events.find((e) => e.type === 'op.finished' && e.opId === opId) as Extract<ClientEvent, { type: 'op.finished' }> | undefined;

// ---------------------------------------------------------------- merge patch
test('merge patch replaces arrays, deletes nulls, merges objects', () => {
  const r = applyMergePatch({ a: 1, b: { c: 1, d: 2 }, e: [1, 2] }, { a: null, b: { c: 9 }, e: [3] });
  ok(JSON.stringify(r) === JSON.stringify({ b: { c: 9, d: 2 }, e: [3] }), JSON.stringify(r));
});

// ---------------------------------------------------------------- client sync
test('client gets snapshot on connect and follows patches', async () => {
  const { client } = setup();
  await until(() => !!client.snapshot, 2000, 'snapshot');
  ok(client.snapshot!.games.length === 6, 'games');
  await client.request('settings.set', { mode: 'expert' });
  await until(() => client.snapshot!.settings.mode === 'expert', 2000, 'patched mode');
});

test('seq gap forces a full resync (never renders a guessed state)', async () => {
  const { client, svc } = setup();
  await until(() => !!client.snapshot);
  const before = client.resyncs;
  svc.gapNext();
  await client.request('settings.set', { theme: 'dark' });
  await until(() => client.resyncs > before, 2000, 'resync');
  await until(() => client.snapshot!.settings.theme === 'dark', 2000, 'theme after resync');
  ok(client.snapshot!.seq === svc.state.seq, `seq ${client.snapshot!.seq} vs ${svc.state.seq}`);
});

// ---------------------------------------------------------------- operations
test('optimize: done → game armed, op removed', async () => {
  const { client, events } = setup();
  await until(() => !!client.snapshot);
  const { opId } = await client.request<{ opId: string }>('session.optimize', { gameId: 'epic:fortnite' });
  await until(() => !!finished(events, opId), 6000, 'op.finished');
  ok(finished(events, opId)!.outcome === 'done', 'outcome');
  await until(() => client.snapshot!.operations.length === 0, 1000, 'ops cleared');
  ok(client.snapshot!.games.find((g) => g.id === 'epic:fortnite')!.status === 'armed', 'armed');
});

test('failing step → failed with actionable message, nothing left running', async () => {
  const { client, svc, events } = setup();
  await until(() => !!client.snapshot);
  svc.scenario.failNext = true;
  const { opId } = await client.request<{ opId: string }>('session.optimize', { gameId: 'epic:fortnite' });
  await until(() => !!finished(events, opId), 6000);
  const f = finished(events, opId)!;
  ok(f.outcome === 'failed' && f.error?.messageKey === 'err.step_failed', JSON.stringify(f));
  await until(() => client.snapshot!.operations.length === 0, 1000, 'ops cleared');
  ok(client.snapshot!.games.find((g) => g.id === 'epic:fortnite')!.status === 'ready', 'not armed after failure');
});

test('deadline exceeded → timed_out (slow scenario)', async () => {
  const { client, svc, events } = setup();
  await until(() => !!client.snapshot);
  svc.scenario.slow = true;
  const { opId } = await client.request<{ opId: string }>('persistent.revert', { tweakId: 'gamedvr_off' });
  // persistent.revert: 2 steps × 6 = ~6.6 s < 15 s → done. Use rescan instead with tight deadline:
  await until(() => !!finished(events, opId), 12000);
  ok(finished(events, opId)!.outcome === 'done', 'revert finishes');
  const r = await client.request<{ opId: string }>('session.optimize', { gameId: 'steam:730' }); // 2.5 s × 6 = 15 s > 15 s
  await until(() => !!finished(events, r.opId), 20000, 'timeout outcome');
  ok(finished(events, r.opId)!.outcome === 'timed_out', finished(events, r.opId)!.outcome);
  ok(client.snapshot!.operations.length === 0, 'no stuck op');
});

test('cancel → cancelled, game not armed', async () => {
  const { client, events } = setup();
  await until(() => !!client.snapshot);
  const { opId } = await client.request<{ opId: string }>('session.optimize', { gameId: 'epic:fortnite' });
  await client.request('op.cancel', { opId });
  await until(() => !!finished(events, opId), 6000);
  ok(finished(events, opId)!.outcome === 'cancelled', 'cancelled');
  ok(client.snapshot!.games.find((g) => g.id === 'epic:fortnite')!.status === 'ready', 'not armed');
});

test('service crash mid-session → link drops, ops cleared, recovered prompt after reconnect', async () => {
  const { client, svc, events } = setup();
  await until(() => !!client.snapshot);
  const { opId } = await client.request<{ opId: string }>('session.optimize', { gameId: 'epic:fortnite' });
  await until(() => !!finished(events, opId), 6000);
  svc.launchGame('epic:fortnite');
  await until(() => !!client.snapshot!.activeSession, 3000, 'session');
  await sleep(1400); // session apply op in flight / done
  svc.crash();
  await until(() => client.link === 'disconnected', 1000, 'disconnected');
  ok(client.snapshot!.operations.length === 0, 'operations cleared on disconnect');
  await until(() => client.link === 'connected' && client.snapshot!.prompts.some((p) => p.kind === 'recovered'), 5000, 'recovered prompt');
  ok(client.snapshot!.activeSession === null, 'session rolled back');
});

test('full session → report event, verdicts, keep-persistent prompt for a proven win', async () => {
  const { client, svc, events } = setup();
  await until(() => !!client.snapshot);
  const { opId } = await client.request<{ opId: string }>('session.optimize', { gameId: 'epic:fortnite' });
  await until(() => !!finished(events, opId), 6000);
  svc.launchGame('epic:fortnite');
  await until(() => (client.snapshot!.activeSession?.tweaksActive ?? 0) > 0, 5000, 'tweaks active');
  svc.closeGame();
  await until(() => events.some((e) => e.type === 'session.report'), 6000, 'report');
  const txId = (events.find((e) => e.type === 'session.report') as { txId: number }).txId;
  const rep = await client.request<{ tweaks: { verdict: string }[]; optimized: { low1: number }; baseline: { low1: number } }>('report.session', { txId });
  ok(rep.tweaks.length > 0, 'verdicts');
  ok(rep.optimized.low1 > rep.baseline.low1, 'hybrid GPU fix should improve 1% low on this preset');
  await until(() => client.snapshot!.prompts.some((p) => p.kind === 'keep_persistent'), 2000, 'keep prompt');
  ok(client.snapshot!.games.find((g) => g.id === 'epic:fortnite')!.status === 'optimized', 'optimized');
  ok(client.snapshot!.suspended.length === 0, 'background resumed after session');
});

test('BIOS coach reboot → reconnect with verified RAM speed and issue resolved', async () => {
  const { client, events } = setup('desktop_entry');
  await until(() => !!client.snapshot);
  await client.request('bios.startFlow', { issueId: 'ram_below_rated' });
  const { opId } = await client.request<{ opId: string }>('bios.rebootToFirmware', { confirm: true });
  await until(() => !!finished(events, opId), 4000);
  await until(() => client.snapshot!.prompts.some((p) => p.kind === 'bios_verified'), 6000, 'bios verified');
  const hw = client.snapshot!.hardware!;
  ok(hw.ramSpeed.value!.configured === 3200, `ram ${hw.ramSpeed.value!.configured}`);
  ok(!client.snapshot!.issues.some((i) => i.id === 'ram_below_rated'), 'issue gone');
  ok(client.snapshot!.coach?.state === 'watching', 'stability watch');
});

test('reboot without confirm is rejected', async () => {
  const { client } = setup('desktop_entry');
  await until(() => !!client.snapshot);
  let code = '';
  try {
    await client.request('bios.rebootToFirmware', {});
  } catch (e) {
    code = (e as { body: { code: string } }).body.code;
  }
  ok(code === 'CONFIRM_REQUIRED', code);
});

// ---------------------------------------------------------------- plan rules
test('kernel anti-cheat locks non-safe tweaks; Easy mode hides disputed ones', async () => {
  const { client } = setup();
  await until(() => !!client.snapshot);
  const easy = await client.request<PlanPreview>('plan.preview', { gameId: 'epic:fortnite' });
  ok(!easy.items.some((i) => i.evidence !== 'proven' || i.risk !== 'low'), 'easy shows only low+proven');
  ok(easy.hiddenInEasy > 0, 'hidden count');
  await client.request('settings.set', { mode: 'expert' });
  const ex = await client.request<PlanPreview>('plan.preview', { gameId: 'epic:fortnite' });
  const timer = ex.items.find((i) => i.tweakId === 'timer_resolution');
  ok(timer?.lock === 'anticheat' && !timer.enabled, 'timer locked by anti-cheat');
  let code = '';
  try {
    await client.request('plan.setToggle', { gameId: 'epic:fortnite', tweakId: 'timer_resolution', enabled: true });
  } catch (e) {
    code = (e as { body: { code: string } }).body.code;
  }
  ok(code === 'TWEAK_LOCKED', code);
  const mc = await client.request<PlanPreview>('plan.preview', { gameId: 'xbox:minecraft' });
  ok(mc.items.find((i) => i.tweakId === 'timer_resolution')?.lock === null, 'no lock without anti-cheat');
});

test('needs_baseline game cannot be optimized before measuring', async () => {
  const { client } = setup();
  await until(() => !!client.snapshot);
  let code = '';
  try {
    await client.request('session.optimize', { gameId: 'riot:valorant' });
  } catch (e) {
    code = (e as { body: { code: string } }).body.code;
  }
  ok(code === 'NEEDS_BASELINE', code);
});

test('simulated gains stay credible (seeded history, 30 runs × 3 presets)', () => {
  for (const p of ['laptop_hybrid', 'desktop_entry', 'apu_lowend'] as const) {
    for (let i = 0; i < 30; i++) {
      const d = new MockService(p).state.reports[0].low1DeltaPct;
      ok(d > 2 && d < 90, `${p}: seeded 1% low delta ${d} out of credible range`);
    }
  }
});

// ---------------------------------------------------------------- i18n
test('i18n: ES and EN have identical key sets', () => {
  const a = Object.keys(es).sort();
  const b = new Set(Object.keys(en));
  const missing = a.filter((k) => !b.has(k));
  const extra = [...b].filter((k) => !(k in es));
  ok(!missing.length && !extra.length, `missing in en: ${missing.join(', ')} | extra in en: ${extra.join(', ')}`);
});

test('i18n: every dynamic key family used by the engine exists', () => {
  const need: string[] = [];
  for (const t of CATALOG) for (const f of ['title', 'why', 'before', 'after']) need.push(`tw.${t.id}.${f}`);
  for (const p of Object.values(PRESETS)) for (const i of p.issues) need.push(`issue.${i.id}.title`, `issue.${i.id}.body`);
  need.push('issue.on_battery.title', 'issue.on_battery.body', 'tw.bios_ram_xmp.title', 'tw.bios_uma_buffer.title');
  for (const v of ['msi', 'asus', 'lenovo', 'generic']) need.push(`coach.vendor.${v}.enter`);
  for (const f of ['ram_xmp', 'uma_buffer']) {
    for (const s of ['explain', 'set']) need.push(`coach.${f}.${s}`);
    for (const v of ['msi', 'asus', 'gigabyte', 'asrock', 'generic']) need.push(`coach.${f}.where.${v}`);
  }
  const src = ['src/mock/service.ts', 'src/ui/views.ts', 'src/ui/overlays.ts', 'src/ui/report.ts', 'src/ui/app.ts']
    .map((f) => readFileSync(new URL(`../${f}`, import.meta.url), 'utf8'))
    .join('\n');
  for (const m of src.matchAll(/['`]((?:op|step|err|notice|prompt|hero|badge|pc|report|plan|hud|coach|settings|tray|dev|link|outcome|pulse|health|verdict|lock|history|scope|risk|evidence|status|bottleneck|fix|nav|mode|common)\.[a-z0-9_.]+)['`]/g))
    need.push(m[1]);
  // Protocol method / event names share the dotted shape but are not i18n keys.
  const protocol = new Set(['op.progress', 'op.finished', 'op.cancel', 'plan.preview', 'plan.setToggle', 'report.session', 'report.health', 'prompt.answer', 'settings.set', 'state.patch', 'state.snapshot']);
  for (const k of protocol) need.splice(0, need.length, ...need.filter((x) => x !== k));
  const missing = [...new Set(need)].filter((k) => !(k in es));
  ok(!missing.length, `missing keys: ${missing.join(', ')}`);
});

test('i18n: every issue id and coach key emitted by the Rust core exists (es + en)', () => {
  const rs = (f: string) => readFileSync(new URL(`../../../crates/overdrive-core/src/${f}`, import.meta.url), 'utf8');
  const issues = rs('issues.rs');
  const coach = rs('coach.rs');
  const ids = new Set<string>();
  for (const m of issues.matchAll(/issue\(\s*"([a-z_]+)"/g)) ids.add(m[1]);
  for (const m of issues.matchAll(/\{\s*"([a-z_]+)"\s*\}/g)) ids.add(m[1]); // if laptop { "thermal_hint" } else { "cpu_throttling" }
  ok(ids.size >= 15, `parsed only ${ids.size} issue ids from issues.rs`);
  const need: string[] = [];
  for (const id of ids) need.push(`issue.${id}.title`, `issue.${id}.body`);
  need.push('issue.ram_below_rated.body.laptop');
  for (const m of coach.matchAll(/"(coach\.[a-z_.]+)"/g)) need.push(m[1]);
  const vendors = ['msi', 'asus', 'gigabyte', 'asrock', 'lenovo', 'hp', 'dell', 'acer', 'generic'];
  const menuVendors = ['msi', 'asus', 'gigabyte', 'asrock', 'generic'];
  ok(coach.includes('coach.vendor.{vk}.enter'), 'coach.rs vendor-enter template changed: update this test');
  for (const v of vendors) need.push(`coach.vendor.${v}.enter`);
  for (const flow of ['ram_xmp', 'uma_buffer']) {
    ok(coach.includes(`coach.${flow}.where.{menu_vk}`), `coach.rs ${flow} where-template changed: update this test`);
    for (const v of menuVendors) need.push(`coach.${flow}.where.${v}`);
  }
  for (const [name, dict] of [['es', es], ['en', en]] as const) {
    const missing = [...new Set(need)].filter((k) => !(k in dict));
    ok(!missing.length, `${name} missing: ${missing.join(', ')}`);
  }
});

test('i18n: every tweak in catalog/tweaks.toml and every exclusion reason has texts (es + en)', () => {
  const root = (f: string) => readFileSync(new URL(`../../../${f}`, import.meta.url), 'utf8');
  const ids = [...root('catalog/tweaks.toml').matchAll(/^id = "([a-z0-9_]+)"/gm)].map((m) => m[1]);
  ok(ids.length >= 10, `parsed ${ids.length} catalog ids`);
  const need: string[] = [];
  for (const id of ids) for (const f of ['title', 'why', 'before', 'after']) need.push(`tw.${id}.${f}`);
  // Exclusion reasons (session.rs) and builder reasons (catalog.rs NotApplicable).
  for (const m of root('crates/overdrive-core/src/session.rs').matchAll(/^\s{4}([A-Z][A-Za-z]+)(?:\(String\))?,$/gm)) {
    need.push(`exclusion.${m[1].replace(/([a-z])([A-Z])/g, '$1_$2').toLowerCase()}`);
  }
  for (const m of root('crates/overdrive-core/src/catalog.rs').matchAll(/NotApplicable\("([a-z0-9_]+)"\)/g)) need.push(`reason.${m[1]}`);
  for (const [name, dict] of [['es', es], ['en', en]] as const) {
    const missing = [...new Set(need)].filter((k) => !(k in dict));
    ok(!missing.length, `${name} missing: ${missing.join(', ')}`);
  }
});

// ---------------------------------------------------------------- run
for (const [name, fn] of tests) {
  try {
    await fn();
    passed++;
    console.log(`  ✓ ${name}`);
  } catch (e) {
    failed++;
    console.log(`  ✗ ${name}\n      ${(e as Error).message}`);
  }
}
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
