// Main views. Pure functions: (engine snapshot + local UI state) → HTML.
import { date, has, pct, t, tweakTitle } from '../i18n/index.ts';
import type { CommunityView, EngineSnapshot, GameCard, PlanItem, PlanPreview, PreviewLine, StructuralIssue } from '../proto/types.ts';
import { shareSection } from './share.ts';
import { attr, esc, initials, tx } from './dom.ts';
import type { UiState } from './state.ts';

// ------------------------------------------------------------------ Games (Layer 1 + 2)
export function gamesView(s: EngineSnapshot, ui: UiState): string {
  const game = s.games.find((g) => g.id === ui.selected) ?? s.games[0];
  return `
  <div class="games-layout">
    <aside class="card">
      <nav class="game-list" aria-label="${tx('nav.games')}">
        ${s.games.map((g) => gameItem(g, g.id === game?.id)).join('')}
      </nav>
    </aside>
    <section>
      ${game ? hero(s, ui, game) : `<div class="card pad">${tx('hero.select')}</div>`}
      ${game && ui.planOpen ? planView(s, ui.plan, game) : ''}
    </section>
  </div>`;
}

function statusChip(g: GameCard): string {
  const cls = g.status === 'optimized' ? 'good' : g.status === 'running' || g.status === 'armed' ? 'accent' : g.status === 'needs_baseline' ? '' : '';
  return `<span class="chip ${cls}">${tx(`status.${g.status}`)}</span>`;
}

function gameItem(g: GameCard, current: boolean): string {
  return `<button class="game-item" data-action="select-game" data-id="${esc(g.id)}" data-key="game-${esc(g.id)}"${attr('aria-current', current ? 'true' : null)}>
    <span class="game-art" style="--h:${g.hue}" aria-hidden="true">${esc(initials(g.name))}</span>
    <span><span class="name">${esc(g.name)}</span><br><span class="meta">${tx(`status.${g.status}`)}${g.measured ? ` · <span class="mono" style="color:${g.measured.low1DeltaPct >= 0 ? 'var(--good)' : 'var(--bad)'}">${esc(pct(g.measured.low1DeltaPct))}</span>` : ''}</span></span>
    ${g.status === 'running' ? '<span class="chip accent"><span class="ico">●</span>LIVE</span>' : ''}
  </button>`;
}

function bottleneckText(g: GameCard): string {
  const main = t(`bottleneck.${g.bottleneck}`);
  const p = g.bottleneckParams ?? {};
  if (p.detail === 'igpu') return `${main} — ${t('bottleneck.detail.igpu')}`;
  if (typeof p.vram === 'number' && g.bottleneck === 'memory_bound') return `${main} — ${t('bottleneck.detail.vram', { vram: p.vram })}`;
  return main;
}

function hero(s: EngineSnapshot, ui: UiState, g: GameCard): string {
  const sess = s.activeSession?.gameId === g.id ? s.activeSession : null;
  const otherRunning = !!s.activeSession && !sess;
  let cta = '';
  let sub = '';
  switch (g.status) {
    case 'needs_baseline':
      cta = `<button class="cta" data-magnetic data-action="benchmark" data-key="cta">${tx('hero.cta.baseline')}</button>`;
      sub = t('hero.baseline.sub', { game: g.name });
      break;
    case 'armed':
      cta = `<button class="cta" disabled data-key="cta">${tx('hero.cta.armed')}</button>`;
      sub = t('hero.armed.sub', { game: g.name });
      break;
    case 'running':
      cta = `<button class="cta" disabled data-key="cta">${tx('hero.cta.running')}</button>`;
      sub = t('hero.running.sub');
      break;
    default:
      cta =
        g.fixesReady > 0
          ? `<button class="cta" data-magnetic data-action="optimize" data-key="cta"${attr('disabled', otherRunning)}>${tx(g.status === 'optimized' ? 'hero.cta.again' : 'hero.cta.optimize')}</button>`
          : `<button class="cta" disabled data-key="cta">${tx('hero.cta.nothing')}</button>`;
      sub = g.status === 'optimized' && g.measured ? t('hero.optimized.sub', { runs: g.measured.runs }) : t('hero.ready.sub', { n: g.fixesReady });
  }

  const pulseLabel =
    s.pulse.kind === 'session'
      ? t('pulse.session')
      : s.pulse.kind === 'thermal'
        ? t('pulse.thermal')
        : s.pulse.kind === 'bottleneck'
          ? t('pulse.bottleneck', { label: t(`bottleneck.${s.pulse.label}`) })
          : t('pulse.idle');

  const readout = sess
    ? `<div class="pulse-readout"><span class="big">${Math.round(sess.fps)}</span><span class="unit">FPS</span><div class="faint mono" style="font-size:12px;margin-top:4px">${tx('hud.low1')} ${Math.round(sess.low1)} · ${esc(sess.frametimeMs.toFixed(1))} ms</div></div>`
    : '';

  const measured = g.measured
    ? `<span class="chip good" title="IC 95 %: ${esc(pct(g.measured.ciLow))} … ${esc(pct(g.measured.ciHigh))}">${tx('badge.measured', { delta: pct(g.measured.low1DeltaPct) })}</span>`
    : `<span class="chip">${tx('badge.unmeasured')}</span>`;

  return `
  <article class="card hero" aria-labelledby="hero-title">
    <div class="hero-head">
      <div>
        <h1 id="hero-title">${esc(g.name)}</h1>
        <div class="sub">${g.antiCheat === 'kernel' ? `🛡 ${tx('hero.anticheat', { ac: g.antiCheatName ?? '' })}` : esc(g.launcher.toUpperCase())}</div>
      </div>
      ${statusChip(g)}
    </div>
    <div class="pulse-wrap" data-slot="pulse" role="img" aria-label="${tx('pulse.label')}: ${esc(pulseLabel)}">
      <div class="pulse-label"><span class="led${s.pulse.kind !== 'idle' ? ' live' : ''}"></span>${esc(pulseLabel)}</div>
      ${readout}
      <div class="pulse-tip" data-pulse-tip></div>
    </div>
    <div class="hero-actions">
      <button class="badge" data-action="toggle-plan" data-key="badge" aria-expanded="${ui.planOpen}">
        <span><span class="k">${tx('badge.limited')}</span><br><span class="v">${esc(bottleneckText(g))}</span></span>
        <span class="sep" aria-hidden="true"></span>
        <span><span class="k">${esc(g.fixesReady === 1 ? t('badge.fixes.one') : t('badge.fixes', { n: g.fixesReady }))}</span><br>${measured}</span>
        <span class="toggle">${tx(ui.planOpen ? 'badge.close' : 'badge.open')} ${ui.planOpen ? '▴' : '▾'}</span>
      </button>
      ${cta}
    </div>
    <div class="hero-note">${esc(sub)}</div>
  </article>`;
}

function planView(s: EngineSnapshot, plan: PlanPreview | null, g: GameCard): string {
  const expert = s.settings.mode === 'expert';
  if (!plan || plan.gameId !== g.id) return `<div class="card plan"><div class="spinner"></div></div>`;
  return `
  <section class="card plan" aria-labelledby="plan-title">
    <div class="plan-head">
      <div><h2 id="plan-title">${tx('plan.title')}</h2><div class="muted" style="font-size:13px;margin-top:2px">${tx('plan.sub')}</div></div>
    </div>
    ${plan.items.map((i) => planRow(i, expert)).join('')}
    ${plan.hiddenInEasy && !expert ? `<div class="faint" style="padding-top:12px;border-top:1px solid var(--line);font-size:12.5px">${tx('plan.hidden_easy', { n: plan.hiddenInEasy })}</div>` : ''}
  </section>`;
}

/** What other installs measured. A gain only appears once enough of them agree. */
function communityChip(c: CommunityView): string {
  const scope = tx(c.sameHardware ? 'community.same_hw' : 'community.similar_hw');
  if (c.status === 'pending') return `<span class="chip">${tx('community.pending', { n: c.installs })}</span>`;
  const cls = c.status === 'verified' ? 'good' : 'bad';
  return `<span class="chip ${cls}" title="${scope}">${tx(`community.${c.status}`, { n: c.installs, delta: c.medianDeltaPct === null ? '' : pct(c.medianDeltaPct) })}</span>`;
}

/** Expert preview: exactly which lines of which file would change (goals.md §5.5). */
function previewDiff(lines: PreviewLine[]): string {
  const byFile = new Map<string, PreviewLine[]>();
  for (const l of lines) byFile.set(l.file, [...(byFile.get(l.file) ?? []), l]);
  const rows = [...byFile]
    .map(([file, ls]) => {
      const body = ls
        .map((l) => {
          const key = `${l.section ? `[${l.section}] ` : ''}${l.key}`;
          const del = l.from === null ? '' : `<div class="del">− ${esc(key)} = ${esc(l.from)}</div>`;
          return `${del}<div class="add">+ ${esc(key)} = ${esc(l.to)}</div>`;
        })
        .join('');
      return `<div class="file">${esc(file)}</div>${body}`;
    })
    .join('');
  return `<div class="diff" aria-label="${tx('plan.preview')}"><span class="faint">${tx('plan.preview')}:</span>${rows}</div>`;
}

function planRow(i: PlanItem, expert: boolean): string {
  const historyCls = i.history === 'kept' ? 'good' : i.history === 'harmful' ? 'bad' : '';
  const chips = [
    `<span class="chip">${tx(`scope.${i.scope}`)}</span>`,
    i.history !== 'untested' ? `<span class="chip ${historyCls}">${tx(`history.${i.history}`)}</span>` : '',
    expert ? `<span class="chip ${i.risk === 'low' ? '' : 'warn'}">${tx(`risk.${i.risk}`)}</span>` : '',
    expert ? `<span class="chip ${i.evidence === 'disputed' ? 'warn' : ''}">${tx(`evidence.${i.evidence}`)}</span>` : '',
    i.requiresReboot ? `<span class="chip warn">${tx('plan.reboot')}</span>` : '',
    i.community ? communityChip(i.community) : '',
  ].join('');
  return `
  <div class="plan-row${i.lock ? ' locked' : ''}">
    <button class="switch" role="switch" aria-checked="${i.enabled}" aria-label="${esc(tweakTitle(i.tweakId))}"
      data-action="toggle-tweak" data-id="${esc(i.tweakId)}" data-enabled="${!i.enabled}" data-key="tw-${esc(i.tweakId)}"${attr('disabled', !!i.lock)}></button>
    <div>
      <h3>${esc(tweakTitle(i.tweakId))}</h3>
      <div class="why">${tx(`tw.${i.tweakId}.why`)}</div>
      <div class="chips">${chips}</div>
      ${i.lock ? `<div class="lock-note">${tx(`lock.${i.lock}`)}</div>` : ''}
      ${expert ? `<div class="tech"><span class="faint">${tx('plan.technical')}:</span> ${esc(i.technical)}</div>` : ''}
      ${expert && i.preview?.length ? previewDiff(i.preview) : ''}
    </div>
    <div class="blueprint" aria-label="${tx('plan.before')} → ${tx('plan.after')}">
      <div class="cell"><span class="lbl">${tx('plan.before')}</span>${tx(i.before)}</div>
      <div class="arrow" aria-hidden="true">→</div>
      <div class="cell after"><span class="lbl">${tx('plan.after')}</span>${tx(i.after)}</div>
    </div>
  </div>`;
}

// ------------------------------------------------------------------ My PC
export function pcView(s: EngineSnapshot): string {
  const hw = s.hardware;
  if (!hw) return `<div class="card pad"><div class="spinner"></div></div>`;
  const inferred = (c: string) => (c === 'inferred' ? ` <span class="chip" title="${tx('pc.inferred')}">≈ ${tx('pc.inferred')}</span>` : '');
  const ram = hw.ramSpeed.value;
  const specs: [string, string][] = [
    ['pc.cpu', esc(hw.cpu.value)],
    ['pc.gpu', esc(hw.gpu.value)],
    ...(hw.gpuSecondary ? ([['pc.gpu2', esc(hw.gpuSecondary.value)]] as [string, string][]) : []),
    [
      'pc.ram',
      `${tx('pc.ram.value', { gb: hw.ramGb.value ?? 0, speed: ram?.configured ?? 0 })}${ram && ram.configured < ram.rated ? ` <span class="chip warn">/ ${ram.rated} MHz</span>` : ''} · ${tx('pc.ram.channels', { n: hw.ramChannels.value ?? 0 })}${inferred(hw.ramChannels.confidence)}`,
    ],
    ['pc.storage', `${esc(hw.storage.value)} · ${tx('pc.storage.free', { pct: hw.storageFreePct.value ?? 0 })}`],
    ['pc.power', `${tx(hw.powerSource === 'ac' ? 'pc.power.ac' : 'pc.power.battery')} · ${esc(hw.os)}`],
  ];
  const h = s.health;
  return `
  <div class="grid" style="grid-template-columns:minmax(0,1fr) minmax(0,1.35fr);align-items:start">
    <section class="card pad">
      <div style="display:flex;justify-content:space-between;align-items:baseline;gap:12px">
        <div><p class="section-title">${tx('pc.title')}</p><h2>${tx(`pc.tier.${hw.tier}`)}</h2></div>
        <button class="btn ghost" data-action="rescan" data-key="rescan">${tx('pc.rescan')}</button>
      </div>
      <div style="margin-top:12px">${specs.map(([k, v]) => `<div class="spec"><div class="k">${tx(k)}</div><div>${v}</div></div>`).join('')}</div>
    </section>
    <section class="card pad">
      <p class="section-title">${tx('pc.advice')}</p>
      <div class="muted" style="font-size:13px;margin-bottom:6px">${tx('pc.advice.sub')}</div>
      ${s.issues.length ? s.issues.map((i) => issueRow(s, i)).join('') : `<div class="empty">${tx('health.rec.none')}</div>`}
    </section>
  </div>
  ${
    h
      ? `<section style="margin-top:16px"><p class="section-title">${tx('pc.health')} · ${esc(h.month)}</p>
    <div class="grid cols-3">
      <div class="card stat"><div class="k">${tx('pc.health.kept')}</div><div class="v">${h.keptFixed}</div></div>
      <div class="card stat"><div class="k">${tx('pc.health.drift')}</div><div class="v">${h.driftRepaired}</div></div>
      <div class="card stat"><div class="k">${tx('pc.health.sessions')}</div><div class="v">${h.sessionsMeasured}</div></div>
    </div>
    ${has(h.recommendationKey) && s.issues.length ? `<div class="muted" style="margin-top:10px;font-size:13px">→ ${tx(h.recommendationKey)}</div>` : ''}</section>`
      : ''
  }
  <div class="grid cols-2" style="margin-top:16px;align-items:start">
    <section class="card pad">
      <p class="section-title">${tx('pc.kept')}</p>
      ${
        s.persistentChanges.length
          ? s.persistentChanges
              .map(
                (c) => `<div class="list-row"><div><div style="font-weight:560">${esc(tweakTitle(c.tweakId))}</div><div class="faint" style="font-size:12px">${tx('pc.kept.since', { date: date(c.since) })}</div></div>
            ${c.source === 'bios_coach' ? '<span class="chip">BIOS</span>' : `<button class="btn" data-action="undo-persistent" data-id="${esc(c.tweakId)}" data-key="undo-${esc(c.tweakId)}">${tx('pc.kept.undo')}</button>`}</div>`,
              )
              .join('')
          : `<div class="empty">${tx('pc.kept.empty')}</div>`
      }
    </section>
    <section class="card pad">
      <p class="section-title">${tx('pc.revert_all')}</p>
      <div class="muted" style="font-size:13px;margin-bottom:14px">${tx('pc.revert_all.sub')}</div>
      <button class="btn danger" data-action="revert-all" data-key="revert-all">${tx('pc.revert_all')}</button>
    </section>
  </div>`;
}

function issueRow(s: EngineSnapshot, i: StructuralIssue): string {
  const params = { ...(i.params ?? {}) };
  let btn = '';
  const f = i.fixableBy;
  if (f.kind === 'software') {
    const done = s.persistentChanges.some((c) => c.tweakId === f.tweakId);
    btn = done ? `<span class="chip good">✓</span>` : `<button class="btn primary" data-action="fix-software" data-id="${esc(f.tweakId)}" data-key="fix-${esc(i.id)}">${tx('fix.software')}</button>`;
  } else if (f.kind === 'bios') btn = `<button class="btn primary" data-action="open-coach" data-id="${esc(i.id)}" data-key="fix-${esc(i.id)}">${tx('fix.bios')}</button>`;
  else btn = `<span class="chip">${tx(f.kind === 'hardware' ? 'fix.hardware' : 'fix.user')}</span>`;
  return `<div class="issue"><span class="bar ${i.impact}" aria-hidden="true"></span>
    <div><h3>${tx(`issue.${i.id}.title`, params)}${i.confidence === 'inferred' ? ` <span class="chip">≈ ${tx('pc.inferred')}</span>` : ''}</h3><p>${tx(`issue.${i.id}.body`, params)}</p></div>
    <div>${btn}</div></div>`;
}

// ------------------------------------------------------------------ History
export function historyView(s: EngineSnapshot): string {
  if (!s.reports.length) return `<div class="card pad empty">${tx('report.empty')}</div>${shareSection(s)}`;
  return `<section class="card pad"><p class="section-title">${tx('nav.history')}</p>
  ${s.reports
    .map((r) => {
      const g = s.games.find((x) => x.id === r.gameId);
      return `<div class="list-row">
        <div style="display:flex;gap:12px;align-items:center">${g ? `<span class="game-art" style="--h:${g.hue}">${esc(initials(g.name))}</span>` : ''}
          <div><div style="font-weight:560">${esc(g?.name ?? r.gameId)}</div><div class="faint" style="font-size:12px">${esc(date(r.endedAt))}</div></div></div>
        <div style="display:flex;gap:12px;align-items:center"><span class="chip ${r.low1DeltaPct > 0 ? 'good' : 'bad'}">${tx('report.low1')} ${esc(pct(r.low1DeltaPct))}</span>
          <button class="btn" data-action="open-report" data-id="${r.txId}" data-key="rep-${r.txId}">${tx('report.title')} →</button></div>
      </div>`;
    })
    .join('')}</section>${shareSection(s)}`;
}
