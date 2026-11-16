// Sharing (goals.md §7): frametime cards, "what we send" previews, help-a-friend codes.
// Everything shown here comes from the service; nothing is sent from this file — the user
// copies a code or opens a pre-filled GitHub issue themselves.
import { num, pct, t, tweakTitle } from '../i18n/index.ts';
import type {
  CardResult,
  EngineSnapshot,
  HelpPreviewResult,
  HelpRequest,
  HelpRequestResult,
  MeasurementsResult,
  ShareCard,
  VerifyResult,
} from '../proto/types.ts';
import { attr, esc, tx } from './dom.ts';

export type ShareState =
  | { kind: 'card'; data: CardResult }
  | { kind: 'measurements'; data: MeasurementsResult }
  | { kind: 'help-request'; data: HelpRequestResult }
  | { kind: 'help-apply'; preview: HelpPreviewResult | null; done: boolean }
  | { kind: 'help-friend'; request: HelpRequest | null; toggles: Record<string, Record<string, boolean>>; note: string; responseCode: string | null }
  | { kind: 'verify'; result: VerifyResult | null };

// ------------------------------------------------------------------ card image

const W = 1200;
const H = 630;
const FONT = "'Segoe UI', 'Inter Variable', Arial, sans-serif";

function path(series: number[], x0: number, y0: number, w: number, h: number, max: number): string {
  if (!series.length) return '';
  const dx = series.length > 1 ? w / (series.length - 1) : 0;
  return series
    .map((v, i) => `${i ? 'L' : 'M'}${(x0 + i * dx).toFixed(1)},${(y0 + h - (Math.min(v, max) / max) * h).toFixed(1)}`)
    .join(' ');
}

/** Self-contained SVG (fixed palette, system fonts) so it rasterises the same everywhere. */
export function cardSvg(c: ShareCard, shortId: string, delta: number): string {
  const all = [...c.baselineSeries, ...c.optimizedSeries];
  const p95 = [...all].sort((a, b) => a - b)[Math.floor(all.length * 0.98)] ?? 20;
  const max = Math.max(10, Math.ceil((p95 * 1.25) / 5) * 5);
  const gx = 64, gy = 250, gw = W - 128, gh = 250;
  const good = delta > 0;
  const kind = c.hw.laptop ? t('share.card.laptop') : t('share.card.desktop');
  const kept = c.kept.slice(0, 3).map((k) => tweakTitle(k)).join(' · ');
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" font-family="${FONT}">
  <rect width="${W}" height="${H}" rx="28" fill="#0d1117"/>
  <text x="64" y="92" fill="#e6edf3" font-size="44" font-weight="700">${esc(c.game)}</text>
  <text x="64" y="134" fill="#8b949e" font-size="24">${esc(t('share.card.sub', { tier: c.hw.tier, kind, n: c.baselineRuns }))}</text>
  <text x="${W - 64}" y="92" fill="${good ? '#3fb950' : '#f85149'}" font-size="56" font-weight="800" text-anchor="end">${esc(pct(delta))}</text>
  <text x="${W - 64}" y="134" fill="#8b949e" font-size="24" text-anchor="end">${esc(t('share.card.low1', { before: num(c.baseline.low1, 0), after: num(c.optimized.low1, 0) }))}</text>
  <line x1="${gx}" y1="${gy + gh}" x2="${gx + gw}" y2="${gy + gh}" stroke="#30363d" stroke-width="2"/>
  <text x="${gx}" y="${gy - 14}" fill="#8b949e" font-size="20">${esc(t('share.card.axis', { max }))}</text>
  <path d="${path(c.baselineSeries, gx, gy, gw, gh, max)}" fill="none" stroke="#3987e5" stroke-width="3" stroke-linejoin="round" opacity="0.9"/>
  <path d="${path(c.optimizedSeries, gx, gy, gw, gh, max)}" fill="none" stroke="#eb6834" stroke-width="3" stroke-linejoin="round"/>
  <rect x="${gx}" y="190" width="18" height="6" rx="3" fill="#3987e5"/><text x="${gx + 28}" y="198" fill="#c9d1d9" font-size="20">${esc(t('report.series.before'))}</text>
  <rect x="${gx + 190}" y="190" width="18" height="6" rx="3" fill="#eb6834"/><text x="${gx + 218}" y="198" fill="#c9d1d9" font-size="20">${esc(t('report.series.after'))}</text>
  <text x="64" y="560" fill="#c9d1d9" font-size="22">${esc(kept)}</text>
  <text x="64" y="596" fill="#6e7681" font-size="20">${esc(t('share.card.footer', { day: c.day, id: shortId }))}</text>
  <text x="${W - 64}" y="596" fill="#e6edf3" font-size="24" font-weight="700" text-anchor="end">overdrive</text>
</svg>`;
}

/** Rasterise the card to PNG and save it (no network). */
export async function downloadPng(svg: string, name: string): Promise<void> {
  const img = new Image();
  const url = URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml' }));
  try {
    await new Promise<void>((ok, err) => {
      img.onload = () => ok();
      img.onerror = () => err(new Error('svg'));
      img.src = url;
    });
    const cv = document.createElement('canvas');
    cv.width = W;
    cv.height = H;
    cv.getContext('2d')!.drawImage(img, 0, 0, W, H);
    const a = document.createElement('a');
    a.href = cv.toDataURL('image/png');
    a.download = name;
    a.click();
  } finally {
    URL.revokeObjectURL(url);
  }
}

// ------------------------------------------------------------------ modal

function codeBox(code: string, copyKey: string): string {
  return `<div class="code-box"><code>${esc(code)}</code></div>
    <div class="row-btns"><button class="btn primary" data-action="share-copy" data-value="${esc(code)}" data-key="${copyKey}">${tx('share.copy')}</button></div>`;
}

function jsonPreview(v: unknown): string {
  return `<details class="what-we-send" open><summary>${tx('share.what_we_send')}</summary><pre>${esc(JSON.stringify(v, null, 2))}</pre></details>`;
}

function input(value: string, placeholderKey: string): string {
  return `<textarea class="code-input" data-input="share" data-key="share-input" rows="4" spellcheck="false" placeholder="${tx(placeholderKey)}">${esc(value)}</textarea>`;
}

function body(st: ShareState, s: EngineSnapshot, inputValue: string): [string, string] {
  switch (st.kind) {
    case 'card': {
      const d = st.data;
      return [
        t('share.card.title'),
        `<div class="card-preview">${cardSvg(d.card, d.shortId, d.low1DeltaPct)}</div>
        <p class="muted">${tx('share.card.hint')}</p>
        <div class="row-btns">
          <button class="btn primary" data-action="share-png" data-key="share-png">${tx('share.card.png')}</button>
          <button class="btn" data-action="share-copy" data-value="${esc(d.code)}" data-key="share-code">${tx('share.card.copy_code')}</button>
        </div>
        ${jsonPreview(d.card)}`,
      ];
    }
    case 'measurements':
      return [
        t('share.community.title'),
        `<p>${tx('share.community.body', { n: st.data.count })}</p>
        <p class="muted">${tx('share.community.how')}</p>
        ${jsonPreview(st.data.preview)}
        <p class="faint mono" style="font-size:12px">${tx('share.community.key', { key: st.data.pubkey.slice(0, 16) })}</p>
        <div class="row-btns"><button class="btn primary" data-action="share-submit" data-key="share-submit">${tx('share.community.open')}</button></div>`,
      ];
    case 'help-request':
      return [
        t('help.request.title'),
        `<p>${tx('help.request.body')}</p>${codeBox(st.data.code, 'help-copy')}${jsonPreview(st.data.preview)}
        <p class="muted">${tx('help.request.next')}</p>`,
      ];
    case 'help-apply': {
      const p = st.preview;
      const list = p
        ? p.response.games
            .map((g) => {
              const name = s.games.find((x) => x.id === g.gameId)?.name ?? g.gameId;
              const rows = [
                ...g.enable.map((id) => `<li class="on">✓ ${esc(tweakTitle(id))}</li>`),
                ...g.disable.map((id) => `<li class="off">– ${esc(tweakTitle(id))}</li>`),
              ].join('');
              return `<h3>${esc(name)}</h3><ul class="reco">${rows || `<li class="faint">${tx('help.apply.none')}</li>`}</ul>`;
            })
            .join('')
        : '';
      return [
        t('help.apply.title'),
        st.done
          ? `<div class="chip good" style="white-space:normal;padding:10px 12px;border-radius:10px">✓ ${tx('help.apply.done', { n: p?.changes ?? 0 })}</div>`
          : `<p>${tx('help.apply.body')}</p>${input(inputValue, 'help.apply.placeholder')}
            <div class="row-btns"><button class="btn" data-action="help-preview" data-key="help-preview">${tx('help.apply.check')}</button></div>
            ${
              p
                ? `${p.response.note ? `<blockquote class="note">${esc(p.response.note)}</blockquote>` : ''}${list}
                  <p class="muted">${tx('help.apply.safe')}</p>
                  <div class="row-btns"><button class="btn primary" data-action="help-apply"${attr('disabled', !p.changes)} data-key="help-apply">${tx('help.apply.btn', { n: p.changes })}</button></div>`
                : ''
            }`,
      ];
    }
    case 'help-friend': {
      const r = st.request;
      if (!r)
        return [
          t('help.friend.title'),
          `<p>${tx('help.friend.body')}</p>${input(inputValue, 'help.friend.placeholder')}
          <div class="row-btns"><button class="btn primary" data-action="help-load" data-key="help-load">${tx('help.friend.open')}</button></div>`,
        ];
      const hw = r.hw;
      const games = r.games
        .map((g) => {
          const rows = g.tweaks
            .map((tw) => {
              const on = st.toggles[g.id]?.[tw.id] ?? tw.enabled;
              return `<div class="plan-row${tw.lock ? ' locked' : ''}">
                <button class="switch" role="switch" aria-checked="${on}" aria-label="${esc(tweakTitle(tw.id))}" data-action="help-toggle" data-id="${esc(g.id)}" data-value="${esc(tw.id)}"${attr('disabled', !!tw.lock)} data-key="hf-${esc(g.id)}-${esc(tw.id)}"></button>
                <div><h3>${esc(tweakTitle(tw.id))}</h3><div class="chips"><span class="chip">${tx(`history.${tw.history}`)}</span>${tw.lock ? `<span class="chip warn">${tx(`lock.${tw.lock}`)}</span>` : ''}</div></div></div>`;
            })
            .join('');
          return `<section style="margin-top:14px"><h3>${esc(g.name)} <span class="faint" style="font-weight:400">· ${tx(`help.friend.ac.${g.anticheat}`)} · ${tx('help.friend.runs', { n: g.baselineRuns })}</span></h3>${rows}</section>`;
        })
        .join('');
      return [
        t('help.friend.title'),
        `<div class="card pad" style="margin-bottom:10px"><b>${esc(hw.cpu)}</b> · ${esc(hw.gpus.join(' + '))} · ${esc(hw.ramGb)} GB${hw.ramMts ? ` @ ${esc(hw.ramMts)}${hw.ramRatedMts && hw.ramRatedMts > hw.ramMts ? ` / ${esc(hw.ramRatedMts)}` : ''} MT/s` : ''} · Tier ${esc(hw.tier)}${hw.laptop ? ` · ${tx('share.card.laptop')}` : ''}
          ${r.issues.length ? `<div class="muted" style="margin-top:6px">${r.issues.map((i) => esc(t(`issue.${i}.title`, { game: t('help.friend.a_game') }))).join(' · ')}</div>` : ''}</div>
        ${games}
        <textarea class="code-input" data-input="note" data-key="help-note" rows="2" maxlength="280" placeholder="${tx('help.friend.note')}">${esc(st.note)}</textarea>
        <div class="row-btns"><button class="btn primary" data-action="help-generate" data-key="help-generate">${tx('help.friend.generate')}</button></div>
        ${st.responseCode ? `<p>${tx('help.friend.send')}</p>${codeBox(st.responseCode, 'hf-copy')}` : ''}`,
      ];
    }
    case 'verify': {
      const r = st.result;
      return [
        t('share.verify.title'),
        `<p>${tx('share.verify.body')}</p>${input(inputValue, 'share.verify.placeholder')}
        <div class="row-btns"><button class="btn primary" data-action="verify-card" data-key="verify-card">${tx('share.verify.btn')}</button></div>
        ${
          r
            ? r.valid && r.card
              ? `<div class="chip good" style="white-space:normal;padding:10px 12px;border-radius:10px">✓ ${tx('share.verify.ok', { id: r.shortId ?? '' })}</div>
                 <div class="card-preview">${cardSvg(r.card, r.shortId ?? '', r.card.baseline.low1 ? Math.round(((r.card.optimized.low1 - r.card.baseline.low1) / r.card.baseline.low1) * 1000) / 10 : 0)}</div>
                 <p class="muted">${tx('share.verify.limits')}</p>`
              : `<div class="chip bad" style="white-space:normal;padding:10px 12px;border-radius:10px">✗ ${tx(r.error ?? 'err.share_bad_code')}</div>`
            : ''
        }`,
      ];
    }
  }
}

export function shareModal(s: EngineSnapshot | null, st: ShareState | null, inputValue: string): string {
  if (!s || !st) return '';
  const [title, content] = body(st, s, inputValue);
  return `<div class="scrim" data-action="close-share-scrim"><div class="card dialog wide share" role="dialog" aria-modal="true" aria-labelledby="share-title" data-stop>
    <div class="dialog-head"><h2 id="share-title">${esc(title)}</h2><button class="btn" data-action="share-close" data-key="share-close">${tx('settings.close')}</button></div>
    ${content}
  </div></div>`;
}

/** Entry points (History view). */
export function shareSection(s: EngineSnapshot): string {
  const expert = s.settings.mode === 'expert';
  const btn = (action: string, key: string, sub: string) =>
    `<button class="list-row share-entry" data-action="${action}" data-key="${action}"><div><div style="font-weight:560">${tx(key)}</div><div class="faint" style="font-size:12px">${tx(sub)}</div></div><span aria-hidden="true">→</span></button>`;
  return `<section class="card pad" style="margin-top:16px"><p class="section-title">${tx('share.section')}</p>
    ${btn('help-request', 'help.request.title', 'help.request.sub')}
    ${btn('help-apply-open', 'help.apply.title', 'help.apply.sub')}
    ${expert ? btn('help-friend-open', 'help.friend.title', 'help.friend.sub') : ''}
    ${s.settings.telemetryOptIn ? btn('share-measurements', 'share.community.title', 'share.community.sub') : ''}
    ${btn('verify-open', 'share.verify.title', 'share.verify.sub')}
  </section>`;
}
