import { t } from '../i18n/index.ts';

const MAP: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

/** Escape any engine-provided or translated text before it goes into HTML. */
export function esc(v: unknown): string {
  return String(v ?? '').replace(/[&<>"']/g, (c) => MAP[c]);
}

/** Translated + escaped. */
export function tx(key: string, params?: Record<string, string | number | undefined>): string {
  return esc(t(key, params));
}

export function attr(name: string, value: string | number | boolean | null | undefined): string {
  if (value === false || value === null || value === undefined) return '';
  if (value === true) return ` ${name}`;
  return ` ${name}="${esc(value)}"`;
}

/** Replace a region's HTML only if it changed; keeps focus on the same logical control. */
export function patch(el: HTMLElement, html: string): boolean {
  if ((el as HTMLElement & { _html?: string })._html === html) return false;
  const active = document.activeElement as HTMLElement | null;
  const key = active && el.contains(active) ? active.dataset.key : undefined;
  el.innerHTML = html;
  (el as HTMLElement & { _html?: string })._html = html;
  if (key) el.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`)?.focus();
  return true;
}

export function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter((w) => /[A-Za-z0-9]/.test(w[0] ?? ''))
    .slice(0, 2)
    .map((w) => w[0]!.toUpperCase())
    .join('');
}
