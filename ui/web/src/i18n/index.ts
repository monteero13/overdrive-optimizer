import type { Locale } from '../proto/types.ts';
import { en } from './en.ts';
import { es } from './es.ts';

const dicts: Record<Locale, Record<string, string>> = { es, en };
let current: Locale = 'es';

export function setLocale(l: Locale): void {
  current = l;
  document.documentElement.lang = l;
}

export function locale(): Locale {
  return current;
}

export function has(key: string): boolean {
  return key in dicts[current] || key in es;
}

/** Translate with {param} interpolation. Missing keys fall back to Spanish, then to the key itself. */
export function t(key: string, params?: Record<string, string | number | undefined>): string {
  const raw = dicts[current][key] ?? es[key] ?? key;
  if (!params) return raw;
  return raw.replace(/\{(\w+)\}/g, (_, k: string) => {
    const v = params[k];
    return v === undefined ? `{${k}}` : typeof v === 'number' ? num(v) : v;
  });
}

export function num(v: number, digits?: number): string {
  return new Intl.NumberFormat(current, { maximumFractionDigits: digits ?? (Math.abs(v) < 10 ? 1 : 0) }).format(v);
}

export function pct(v: number): string {
  const s = new Intl.NumberFormat(current, { maximumFractionDigits: 1, signDisplay: 'always' }).format(v);
  return current === 'es' ? `${s} %` : `${s}%`;
}

export function date(ts: number): string {
  return new Intl.DateTimeFormat(current, { day: 'numeric', month: 'short' }).format(ts);
}

/** Title of a tweak id, including BIOS-coach pseudo-tweaks. */
export function tweakTitle(id: string): string {
  return t(`tw.${id}.title`);
}
