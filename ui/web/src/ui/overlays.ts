// Chrome around the views: top bar, operation progress, prompts, toasts,
// connection banner, live HUD, BIOS coach, settings, confirm, tray, dev panel.
import { date, pct, t, tweakTitle } from '../i18n/index.ts';
import type { EngineSnapshot, OperationView, Prompt } from '../proto/types.ts';
import type { PresetId } from '../mock/presets.ts';
import type { Scenario } from '../mock/service.ts';
import { attr, esc, tx } from './dom.ts';
import type { UiState } from './state.ts';

export function topbar(s: EngineSnapshot | null, ui: UiState): string {
  const nav = (['games', 'pc', 'history'] as const)
    .map((v) => `<button data-action="nav" data-id="${v}" data-key="nav-${v}"${attr('aria-current', ui.view === v ? 'page' : null)}>${tx(`nav.${v}`)}</button>`)
    .join('');
  const bg = s?.operations.filter((o) => !o.blocking) ?? [];
  const mode = s?.settings.mode ?? 'easy';
  return `
  <header class="topbar">
    <div class="wordmark"><span class="dot" aria-hidden="true"></span>${tx('app.name')}</div>
    <nav class="nav" aria-label="main">${nav}</nav>
    <div class="spacer"></div>
    ${bg.length ? `<div class="activity" role="status"><span class="spinner"></span>${esc(opLabel(bg[0]))}${bg.length > 1 ? ` +${bg.length - 1}` : ''}</div>` : ''}
    <div class="seg" role="group" aria-label="${tx('settings.mode')}">
      <button data-action="set" data-id="mode" data-value="easy" data-key="mode-easy" aria-pressed="${mode === 'easy'}">${tx('mode.easy')}</button>
      <button data-action="set" data-id="mode" data-value="expert" data-key="mode-expert" aria-pressed="${mode === 'expert'}">${tx('mode.expert')}</button>
    </div>
    <button class="icon-btn" data-action="open-settings" data-key="settings" aria-label="${tx('nav.settings')}" title="${tx('nav.settings')}">⚙</button>
  </header>`;
}

const opLabel = (o: OperationView) => t(o.labelKey, labelParams(o.labelParams));
const labelParams = (p?: Record<string, string | number>) => {
  if (!p) return p;
  const out: Record<string, string | number> = { ...p };
  if (typeof out.tweak === 'string') out.tweak = tweakTitle(out.tweak);
  return out;
};

/** Modal progress — rendered ONLY for blocking operations present in the snapshot. */
export function opModal(s: EngineSnapshot | null): string {
  const op = s?.operations.find((o) => o.blocking);
  if (!op) return '';
  const secs = Math.max(0, Math.round((Date.now() - op.startedAt) / 1000));
  const w = Math.round((op.step / Math.max(1, op.total)) * 100);
  return `
  <div class="scrim">
    <div class="card dialog" role="alertdialog" aria-modal="true" aria-labelledby="op-title" aria-describedby="op-step">
      <h2 id="op-title">${esc(opLabel(op))}</h2>
      <div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="${op.total}" aria-valuenow="${op.step}"><i style="width:${w}%"></i></div>
      <div id="op-step" class="muted" aria-live="polite">${tx(op.stepKey, labelParams(op.stepParams))}</div>
      <div class="op-meta" style="margin-top:14px"><span>${tx('op.step', { step: op.step, total: op.total })}</span><span>${tx('op.elapsed', { s: secs })} / ${Math.round(op.deadlineMs / 1000)} s</span></div>
      ${op.cancellable ? `<div style="display:flex;justify-content:flex-end;margin-top:16px"><button class="btn" data-action="cancel-op" data-id="${esc(op.opId)}" data-key="cancel-op"${attr('disabled', op.state === 'cancelling')}>${tx('op.cancel')}</button></div>` : ''}
    </div>
  </div>`;
}

function promptCard(s: EngineSnapshot, p: Prompt): string {
  const ok = `<button class="btn primary" data-action="answer" data-id="${esc(p.promptId)}" data-value="ok" data-key="p-${esc(p.promptId)}">${tx('prompt.ok')}</button>`;
  switch (p.kind) {
    case 'keep_persistent': {
      const g = s.games.find((x) => x.id === p.gameId);
      return `<div class="card prompt" role="dialog" aria-labelledby="pt-${esc(p.promptId)}"><h3 id="pt-${esc(p.promptId)}">${tx('prompt.keep.title')}</h3>
        <p>${tx('prompt.keep.body', { tweak: tweakTitle(p.tweakId), game: g?.name ?? '', delta: pct(p.deltaPct), runs: p.runs })}</p>
        <div class="row"><button class="btn ghost" data-action="answer" data-id="${esc(p.promptId)}" data-value="no">${tx('prompt.keep.no')}</button>
        <button class="btn primary" data-action="answer" data-id="${esc(p.promptId)}" data-value="yes" data-key="p-${esc(p.promptId)}">${tx('prompt.keep.yes')}</button></div></div>`;
    }
    case 'recovered':
      return `<div class="card prompt"><h3>${tx('prompt.recovered.title')}</h3><p>${tx('prompt.recovered.body')}</p><div class="row">${ok}</div></div>`;
    case 'drift':
      return `<div class="card prompt"><h3>${tx('prompt.drift.title')}</h3><p>${tx(`prompt.drift.${p.cause}`, { tweak: tweakTitle(p.tweakId) })}</p><div class="row">${ok}</div></div>`;
    case 'bios_verified':
      return `<div class="card prompt"><h3>${tx('prompt.bios.title')}</h3><p>${tx(p.flowId === 'uma_buffer' ? 'prompt.bios.body.uma' : 'prompt.bios.body', { before: p.before, after: p.after })}</p><div class="row">${ok}</div></div>`;
    case 'notice':
      return `<div class="card prompt"><p style="margin-top:0">${tx(p.messageKey, p.params)}</p><div class="row">${ok}</div></div>`;
  }
}

export function promptsAndToasts(s: EngineSnapshot | null, ui: UiState): string {
  const prompts = s?.prompts.slice(0, 2).map((p) => promptCard(s, p)) ?? [];
  const toasts = ui.toasts.map(
    (tt) => `<div class="card toast ${tt.tone}" role="status"><div style="flex:1">${esc(tt.text)}</div><button class="btn ghost" style="padding:2px 8px" data-action="dismiss-toast" data-id="${tt.id}" aria-label="${tx('common.close')}">×</button></div>`,
  );
  if (!prompts.length && !toasts.length) return '';
  return `<div class="prompts" aria-live="polite">${toasts.join('')}${prompts.join('')}</div>`;
}

export function banner(link: 'connected' | 'disconnected', hadSnapshot: boolean): string {
  if (link === 'connected' || !hadSnapshot) return '';
  return `<div class="card banner" role="alert"><span class="spinner"></span><div><b>${tx('link.down')}</b><div class="muted" style="font-size:12.5px">${tx('link.down.sub')}</div></div></div>`;
}

function spark(values: number[]): string {
  if (values.length < 2) return '';
  const max = Math.max(...values) * 1.05;
  const min = Math.min(...values) * 0.95;
  const d = values
    .map((v, i) => `${i ? 'L' : 'M'}${((i / (values.length - 1)) * 200).toFixed(1)},${(44 - ((v - min) / (max - min || 1)) * 40 - 2).toFixed(1)}`)
    .join('');
  return `<svg viewBox="0 0 200 44" preserveAspectRatio="none" aria-hidden="true"><path d="${d}" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linejoin="round" vector-effect="non-scaling-stroke"/></svg>`;
}

export function hud(s: EngineSnapshot | null): string {
  const a = s?.activeSession;
  if (!s || !a) return '';
  const g = s.games.find((x) => x.id === a.gameId);
  const expert = s.settings.mode === 'expert';
  return `<aside class="card hud" aria-label="HUD">
    <div class="row"><span style="font-weight:600">${esc(g?.name ?? '')}</span><span class="chip accent">LIVE</span></div>
    <div class="row" style="margin-top:8px"><span class="big">${Math.round(a.fps)}</span><span class="faint mono">${tx('hud.fps')}</span></div>
    <div class="kv"><span class="muted">${tx('hud.low1')}</span><span class="mono">${Math.round(a.low1)}</span>
      <span class="muted">${tx('hud.ft')}</span><span class="mono">${esc(a.frametimeMs.toFixed(1))} ms</span>
      ${expert ? `<span class="muted">${tx('hud.gpubusy')}</span><span class="mono">${esc(a.gpuBusyMs.toFixed(1))} ms</span>` : ''}</div>
    ${spark(a.recent.slice(-60))}
    <div class="faint" style="font-size:12px;margin-top:8px">${a.tweaksActive ? tx('hud.active', { n: a.tweaksActive }) : tx('hud.none')}</div>
    <div class="faint" style="font-size:11px">${tx('hud.paused')}</div>
  </aside>`;
}

export function coachModal(s: EngineSnapshot | null, ui: UiState): string {
  const c = s?.coach;
  if (!ui.coachOpen || !c) return '';
  const vendor = s?.hardware?.boardVendor.value ?? 'generic';
  return `<div class="scrim" data-action="close-coach-scrim"><div class="card dialog" role="dialog" aria-modal="true" aria-labelledby="coach-title" data-stop>
    <div class="dialog-head"><div><p class="section-title">${esc(vendor)}</p><h2 id="coach-title">${tx('coach.title')}</h2><div class="muted" style="font-size:13px;margin-top:2px">${tx('coach.sub')}</div></div>
      <button class="btn" data-action="close-coach" data-key="close-coach">${tx('coach.close')}</button></div>
    ${
      c.state === 'watching' || c.state === 'done'
        ? `<div class="chip good" style="white-space:normal;padding:10px 12px;border-radius:10px">✓ ${tx('coach.watching', { date: c.watchUntil ? date(c.watchUntil) : '' })}</div>`
        : `<ol class="steps">${c.steps
            .map((st) =>
              st.kind === 'reboot'
                ? `<li><div>${tx(st.key)}<div style="margin-top:10px"><button class="btn primary" data-action="coach-reboot" data-key="coach-reboot">${tx('coach.reboot_btn')}</button></div></div></li>`
                : `<li><div>${tx(st.key, { ...(st.params ?? {}), vendor })}</div></li>`,
            )
            .join('')}</ol>`
    }
  </div></div>`;
}

export function settingsModal(s: EngineSnapshot | null, ui: UiState): string {
  if (!ui.settingsOpen || !s) return '';
  const st = s.settings;
  const seg = (id: string, cur: string, opts: [string, string][]) =>
    `<div class="seg">${opts.map(([v, k]) => `<button data-action="set" data-id="${id}" data-value="${v}" aria-pressed="${cur === v}">${tx(k)}</button>`).join('')}</div>`;
  return `<div class="scrim" data-action="close-settings-scrim"><div class="card dialog" role="dialog" aria-modal="true" aria-labelledby="set-title" data-stop>
    <div class="dialog-head"><h2 id="set-title">${tx('settings.title')}</h2><button class="btn" data-action="close-settings" data-key="close-settings">${tx('settings.close')}</button></div>
    <div class="field"><div class="k">${tx('settings.mode')}</div>${seg('mode', st.mode, [['easy', 'mode.easy'], ['expert', 'mode.expert']])}<div class="hint">${tx(`settings.mode.${st.mode}`)}</div></div>
    <div class="field"><div class="k">${tx('settings.language')}</div><div class="seg"><button data-action="set" data-id="locale" data-value="es" aria-pressed="${st.locale === 'es'}">Español</button><button data-action="set" data-id="locale" data-value="en" aria-pressed="${st.locale === 'en'}">English</button></div></div>
    <div class="field"><div class="k">${tx('settings.theme')}</div>${seg('theme', st.theme, [['auto', 'settings.theme.auto'], ['dark', 'settings.theme.dark'], ['light', 'settings.theme.light']])}</div>
    <div class="field"><div class="k">${tx('settings.motion')}</div>${seg('reduceMotion', st.reduceMotion, [['auto', 'settings.motion.auto'], ['on', 'settings.motion.on'], ['off', 'settings.motion.off']])}</div>
    <div class="field"><div style="display:flex;justify-content:space-between;gap:16px;align-items:center"><div class="k" style="margin:0">${tx('settings.telemetry')}</div>
      <button class="switch" role="switch" aria-checked="${st.telemetryOptIn}" aria-label="${tx('settings.telemetry')}" data-action="set" data-id="telemetryOptIn" data-value="${!st.telemetryOptIn}"></button></div>
      <div class="hint">${tx('settings.telemetry.sub')}</div></div>
  </div></div>`;
}

export function confirmModal(ui: UiState): string {
  if (!ui.confirm) return '';
  return `<div class="scrim"><div class="card dialog" role="alertdialog" aria-modal="true" aria-labelledby="cf-title">
    <h2 id="cf-title" style="margin-bottom:18px">${esc(ui.confirm.text)}</h2>
    <div style="display:flex;justify-content:flex-end;gap:8px"><button class="btn ghost" data-action="confirm-no" data-key="cf-no">${tx('prompt.keep.no')}</button>
    <button class="btn primary" data-action="confirm-yes" data-key="cf-yes">${tx('prompt.ok')}</button></div></div></div>`;
}

export function tray(s: EngineSnapshot | null, ui: UiState): string {
  if (!ui.trayOpen || !s) return '';
  const a = s.activeSession;
  const g = a ? s.games.find((x) => x.id === a.gameId) : null;
  return `<div class="card tray" role="menu" aria-label="${tx('tray.title')}">
    <div class="head"><span class="wordmark"><span class="dot"></span></span><div><div style="font-weight:600">${a ? tx('tray.session', { game: g?.name ?? '' }) : tx('tray.idle')}</div>
      ${s.suspended.length ? `<div class="faint" style="font-size:12px">${tx('tray.suspended', { n: s.suspended.length })}</div>` : ''}</div></div>
    <button role="menuitem" data-action="tray-open">${tx('tray.open')}</button>
    <button role="menuitem" data-action="resume-all"${attr('disabled', !s.suspended.length)}>${tx('tray.resume')}</button>
    <button role="menuitem" data-action="revert-all">${tx('tray.revert')}</button>
  </div>`;
}

export function devPanel(s: EngineSnapshot | null, ui: UiState, sc: Scenario, preset: PresetId, resyncs: number, thermal: boolean): string {
  const running = !!s?.activeSession;
  const opts = (['laptop_hybrid', 'desktop_entry', 'apu_lowend'] as PresetId[])
    .map((p) => `<option value="${p}"${attr('selected', p === preset)}>${tx(`dev.preset.${p}`)}</option>`)
    .join('');
  return `<details class="card dev"${attr('open', ui.devOpen)} data-dev>
    <summary data-action="dev-toggle">${tx('dev.title')} <span class="faint">${tx('dev.stats', { seq: s?.seq ?? 0, r: resyncs })}</span></summary>
    <div class="body">
      <div class="btns">
        <button class="btn primary" data-action="dev-launch"${attr('disabled', running || !s)}>▶ ${tx('dev.launch')}</button>
        <button class="btn" data-action="dev-close"${attr('disabled', !running)}>■ ${tx('dev.close')}</button>
      </div>
      <label><input type="checkbox" data-action="dev-slow"${attr('checked', sc.slow)}> ${tx('dev.slow')}</label>
      <label><input type="checkbox" data-action="dev-fail"${attr('checked', sc.failNext)}> ${tx('dev.fail')}</label>
      <label><input type="checkbox" data-action="dev-thermal"${attr('checked', thermal)}${attr('disabled', running)}> ${tx('dev.thermal')}</label>
      <label><input type="checkbox" data-action="dev-battery"${attr('checked', s?.hardware?.powerSource === 'battery')}> ${tx('dev.battery')}</label>
      <div class="btns">
        <button class="btn" data-action="dev-crash">${tx('dev.crash')}</button>
        <button class="btn" data-action="dev-gap">${tx('dev.gap')}</button>
        <button class="btn" data-action="dev-drift" data-value="windows_update">${tx('dev.drift_wu')}</button>
        <button class="btn" data-action="dev-drift" data-value="driver_update">${tx('dev.drift_drv')}</button>
        <button class="btn" data-action="dev-tray">${tx('dev.tray')}</button>
      </div>
      <div class="faint">${tx('dev.preset')}</div>
      <select data-action="dev-preset" aria-label="${tx('dev.preset')}">${opts}</select>
    </div>
  </details>`;
}
