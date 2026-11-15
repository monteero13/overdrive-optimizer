// Layer 3 — post-session report. Frametime chart: 2 series, one axis (ms),
// legend + direct end labels, crosshair tooltip, keyboard support, table view.
import { num, pct, t, tweakTitle } from '../i18n/index.ts';
import type { EngineSnapshot, Metrics, SessionReport } from '../proto/types.ts';
import { esc, tx } from './dom.ts';

const VW = 1000;
const VH = 240;

function niceMax(v: number): number {
  const step = v > 60 ? 20 : v > 30 ? 10 : 5;
  return Math.ceil(v / step) * step;
}

function delta(before: number, after: number, higherIsBetter: boolean): string {
  if (!before) return '';
  const d = (after / before - 1) * 100;
  const good = higherIsBetter ? d > 0.5 : d < -0.5;
  const bad = higherIsBetter ? d < -0.5 : d > 0.5;
  return `<div class="d ${good ? 'up' : bad ? 'down' : ''}">${esc(pct(Math.round(d * 10) / 10))}</div>`;
}

function tile(key: string, b: number, a: number, unit: string, higherIsBetter: boolean, hintKey?: string): string {
  return `<div class="card stat"><div class="k">${tx(key)}</div>
    <div class="v">${esc(num(a, 1))}<span class="faint" style="font-size:13px;margin-left:4px">${esc(unit)}</span></div>
    <div class="faint mono" style="font-size:12px">${tx('report.series.before')} ${esc(num(b, 1))}</div>
    ${delta(b, a, higherIsBetter)}
    ${hintKey ? `<div class="hint">${tx(hintKey)}</div>` : ''}</div>`;
}

export function reportView(s: EngineSnapshot, r: SessionReport, table: boolean): string {
  const g = s.games.find((x) => x.id === r.gameId);
  const B = r.baseline;
  const A = r.optimized;
  return `
  <div class="scrim" data-action="close-report-scrim">
    <div class="card dialog wide" role="dialog" aria-modal="true" aria-labelledby="rep-title" data-stop>
      <div class="dialog-head">
        <div><p class="section-title">${tx('report.title')}</p><h2 id="rep-title">${esc(g?.name ?? r.gameId)}</h2>
          <div class="muted" style="font-size:13px;margin-top:2px">${tx('report.vs', { n: r.baselineRuns })}</div></div>
        <button class="btn" data-action="close-report" data-key="close-report">${tx('report.close')}</button>
      </div>
      <div class="grid cols-4">
        ${tile('report.low1', B.low1, A.low1, 'FPS', true, 'report.low1.hint')}
        ${tile('report.low01', B.low01, A.low01, 'FPS', true)}
        ${tile('report.avg', B.avgFps, A.avgFps, 'FPS', true)}
        ${tile('report.stdev', B.stdevMs, A.stdevMs, 'ms', false, 'report.stdev.hint')}
      </div>
      <section style="margin-top:20px">
        <div style="display:flex;justify-content:space-between;align-items:center;gap:12px;flex-wrap:wrap">
          <h3>${tx('report.chart')}</h3>
          <div style="display:flex;gap:14px;align-items:center">
            <div class="legend">
              <span><span class="key" style="background:var(--series-1)"></span>${tx('report.series.before')}</span>
              <span><span class="key" style="background:var(--series-2)"></span>${tx('report.series.after')}</span>
            </div>
            <button class="btn ghost" data-action="report-table" data-key="rep-table">${tx(table ? 'report.chart_view' : 'report.table')}</button>
          </div>
        </div>
        ${table ? metricsTable(B, A) : chart(r)}
      </section>
      <section style="margin-top:20px">
        <h3 style="margin-bottom:6px">${tx('report.what')}</h3>
        ${r.tweaks
          .map((tw) => {
            const ico = tw.verdict === 'kept' ? '✓' : tw.verdict === 'harmful' ? '!' : '–';
            return `<div class="verdict"><span class="ico ${tw.verdict}" aria-hidden="true">${ico}</span>
              <div><b>${esc(tweakTitle(tw.tweakId))}</b><div class="muted">${tx(`verdict.${tw.verdict}`, { delta: tw.deltaPct === null ? '' : pct(tw.deltaPct) })}</div></div></div>`;
          })
          .join('')}
      </section>
    </div>
  </div>`;
}

function metricsTable(B: Metrics, A: Metrics): string {
  const rows: [string, keyof Metrics, string][] = [
    ['report.avg', 'avgFps', 'FPS'],
    ['report.low1', 'low1', 'FPS'],
    ['report.low01', 'low01', 'FPS'],
    ['p50', 'p50Ms', 'ms'],
    ['p99', 'p99Ms', 'ms'],
    ['report.stdev', 'stdevMs', 'ms'],
    ['report.stutters', 'stutters', ''],
  ];
  return `<table class="data" style="margin-top:10px"><thead><tr><th>${tx('report.metric')}</th><th>${tx('report.series.before')}</th><th>${tx('report.series.after')}</th></tr></thead><tbody>
  ${rows.map(([k, f, u]) => `<tr><th scope="row" style="font-weight:500">${k.startsWith('report.') ? tx(k) : esc(k)}</th><td>${esc(num(B[f], 1))} ${esc(u)}</td><td>${esc(num(A[f], 1))} ${esc(u)}</td></tr>`).join('')}
  </tbody></table>`;
}

function path(series: number[], max: number): string {
  return series
    .map((v, i) => `${i ? 'L' : 'M'}${((i / (series.length - 1)) * VW).toFixed(1)},${(VH - (v / max) * VH).toFixed(1)}`)
    .join('');
}

function chart(r: SessionReport): string {
  const max = niceMax(Math.max(...r.baselineSeries, ...r.optimizedSeries) * 1.08);
  const ticks = [0, max / 4, max / 2, (3 * max) / 4, max];
  const endB = r.baselineSeries.at(-1)!;
  const endA = r.optimizedSeries.at(-1)!;
  let yB = (1 - endB / max) * 100;
  let yA = (1 - endA / max) * 100;
  if (Math.abs(yB - yA) < 9) {
    const midY = (yA + yB) / 2;
    if (yB < yA) [yB, yA] = [midY - 4.5, midY + 4.5];
    else [yA, yB] = [midY - 4.5, midY + 4.5];
  }
  const mins = Math.max(1, Math.round(r.gameplaySecs / 60));
  return `
  <div class="chart-wrap" data-chart tabindex="0" role="img" aria-label="${tx('report.chart')}" style="padding:0 118px 22px 34px">
    <div style="position:relative">
      ${ticks
        .map((tk) => `<div class="faint mono" style="position:absolute;left:-34px;top:calc(${(1 - tk / max) * 100}% - 8px);font-size:11px">${esc(num(tk, 0))}</div>`)
        .join('')}
      <svg viewBox="0 0 ${VW} ${VH}" preserveAspectRatio="none" aria-hidden="true">
        ${ticks.map((tk) => `<line x1="0" x2="${VW}" y1="${VH - (tk / max) * VH}" y2="${VH - (tk / max) * VH}" stroke="var(--line)" stroke-width="1" vector-effect="non-scaling-stroke"/>`).join('')}
        <path d="${path(r.baselineSeries, max)}" fill="none" stroke="var(--series-1)" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" vector-effect="non-scaling-stroke"/>
        <path d="${path(r.optimizedSeries, max)}" fill="none" stroke="var(--series-2)" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" vector-effect="non-scaling-stroke"/>
      </svg>
      <div class="end-label" style="top:calc(${yB}% - 9px);color:var(--text-2)"><span class="key" style="background:var(--series-1)"></span>${tx('report.series.before')}</div>
      <div class="end-label" style="top:calc(${yA}% - 9px);color:var(--text-1);font-weight:560"><span class="key" style="background:var(--series-2)"></span>${tx('report.series.after')}</div>
      <div data-xhair style="position:absolute;top:0;bottom:0;width:1px;background:var(--line-strong);display:none"></div>
      <div data-hit style="position:absolute;inset:0;cursor:crosshair"></div>
      <div class="tooltip" data-tip></div>
    </div>
    <div class="faint mono" style="display:flex;justify-content:space-between;font-size:11px;margin-top:6px"><span>0:00</span><span>${mins} min</span></div>
  </div>`;
}

/** Crosshair + tooltip + keyboard. Values via textContent only. */
export function wireReportChart(root: HTMLElement, r: SessionReport): void {
  const wrap = root.querySelector<HTMLElement>('[data-chart]');
  if (!wrap || (wrap as HTMLElement & { _wired?: boolean })._wired) return;
  (wrap as HTMLElement & { _wired?: boolean })._wired = true;
  const hit = wrap.querySelector<HTMLElement>('[data-hit]')!;
  const xhair = wrap.querySelector<HTMLElement>('[data-xhair]')!;
  const tip = wrap.querySelector<HTMLElement>('[data-tip]')!;
  const n = r.baselineSeries.length;
  let idx = n - 1;
  const mins = r.gameplaySecs / 60;

  const show = (i: number) => {
    idx = Math.max(0, Math.min(n - 1, i));
    const xf = idx / (n - 1);
    xhair.style.display = 'block';
    xhair.style.left = `${xf * 100}%`;
    tip.replaceChildren();
    const time = document.createElement('div');
    time.className = 'faint mono';
    const m = xf * mins;
    time.textContent = `${Math.floor(m)}:${String(Math.round((m % 1) * 60)).padStart(2, '0')}`;
    tip.append(time);
    for (const [label, val, color] of [
      [t('report.series.after'), r.optimizedSeries[idx], 'var(--series-2)'],
      [t('report.series.before'), r.baselineSeries[idx], 'var(--series-1)'],
    ] as const) {
      const row = document.createElement('div');
      row.className = 'r';
      const key = document.createElement('span');
      key.className = 'key';
      key.style.background = color;
      const b = document.createElement('b');
      b.textContent = `${num(val, 1)} ms`;
      const l = document.createElement('span');
      l.className = 'muted';
      l.textContent = label;
      row.append(key, b, l);
      tip.append(row);
    }
    tip.classList.add('on');
    tip.style.left = xf > 0.7 ? '' : `calc(${xf * 100}% + 12px)`;
    tip.style.right = xf > 0.7 ? `calc(${(1 - xf) * 100}% + 12px)` : '';
    tip.style.top = '8px';
  };
  const hide = () => {
    xhair.style.display = 'none';
    tip.classList.remove('on');
  };
  hit.addEventListener('pointermove', (e) => {
    const rect = hit.getBoundingClientRect();
    show(Math.round(((e.clientX - rect.left) / rect.width) * (n - 1)));
  });
  hit.addEventListener('pointerleave', hide);
  wrap.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
      e.preventDefault();
      show(idx + (e.key === 'ArrowRight' ? 1 : -1) * (e.shiftKey ? 10 : 1));
    } else if (e.key === 'Escape') hide();
  });
  wrap.addEventListener('blur', hide);
}
