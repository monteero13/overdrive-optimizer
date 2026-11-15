// The "Pulse": a generative waveform that reflects real engine state.
//  idle        → calm, slow, neutral
//  bottleneck  → flow pinched at the bottleneck, amplitude by intensity
//  thermal     → warm, jittery
//  session     → draws the REAL frametime series; no animation loop while a game runs
//                (goals.md §6: the UI must not cost FPS)
// Canvas 2D, DPR capped at 1.5 and 30 fps cap so it stays cheap on iGPUs.

import type { PulseState, SessionView } from '../proto/types.ts';

interface Colors {
  ink: string;
  faint: string;
  accent: string;
  warn: string;
}

export class Pulse {
  readonly canvas = document.createElement('canvas');
  private ctx = this.canvas.getContext('2d')!;
  private state: PulseState = { kind: 'idle' };
  private session: SessionView | null = null;
  private reduced = false;
  private raf = 0;
  private last = 0;
  private t0 = performance.now();
  private colors: Colors = { ink: '#888', faint: '#555', accent: '#f60', warn: '#ea0' };
  private ro = new ResizeObserver(() => this.resize());
  private w = 0;
  private h = 0;
  /** Amplitude eases toward the target so state changes feel liquid. */
  private amp = 0.2;

  constructor() {
    this.canvas.setAttribute('aria-hidden', 'true');
    this.ro.observe(this.canvas);
    document.addEventListener('visibilitychange', () => this.schedule());
  }

  refreshColors(): void {
    const cs = getComputedStyle(document.documentElement);
    this.colors = {
      ink: cs.getPropertyValue('--text-2').trim(),
      faint: cs.getPropertyValue('--text-3').trim(),
      accent: cs.getPropertyValue('--accent').trim(),
      warn: cs.getPropertyValue('--warn').trim(),
    };
    this.draw(performance.now());
  }

  set(state: PulseState, session: SessionView | null, reducedMotion: boolean): void {
    this.state = state;
    this.session = session;
    this.reduced = reducedMotion;
    this.schedule();
    this.draw(performance.now()); // immediate frame (also the only frame when static)
  }

  /** Value under the pointer for the hover readout: frametime in session, else null. */
  valueAt(xFrac: number): number | null {
    const r = this.session?.recent;
    if (!r?.length) return null;
    return r[Math.min(r.length - 1, Math.max(0, Math.round(xFrac * (r.length - 1))))];
  }

  private animated(): boolean {
    return !this.reduced && this.state.kind !== 'session' && !document.hidden && this.canvas.isConnected;
  }

  private schedule(): void {
    if (this.animated()) {
      if (!this.raf) this.raf = requestAnimationFrame((t) => this.loop(t));
    } else if (this.raf) {
      cancelAnimationFrame(this.raf);
      this.raf = 0;
    }
  }

  private loop(t: number): void {
    this.raf = 0;
    if (t - this.last >= 33) {
      this.last = t;
      this.draw(t);
    }
    this.schedule();
  }

  private resize(): void {
    const dpr = Math.min(1.5, window.devicePixelRatio || 1);
    const r = this.canvas.getBoundingClientRect();
    this.w = Math.max(1, r.width);
    this.h = Math.max(1, r.height);
    this.canvas.width = Math.round(this.w * dpr);
    this.canvas.height = Math.round(this.h * dpr);
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.draw(performance.now());
    this.schedule();
  }

  private draw(now: number): void {
    const { ctx, w, h } = this;
    if (!w || !h) return;
    ctx.clearRect(0, 0, w, h);
    if (this.state.kind === 'session') return this.drawSession();

    const t = this.reduced ? 0 : (now - this.t0) / 1000;
    const s = this.state;
    const target = s.kind === 'idle' ? 0.16 : s.kind === 'thermal' ? 0.3 : 0.18 + s.intensity * 0.2;
    this.amp += (target - this.amp) * (this.reduced ? 1 : 0.05);
    const speed = s.kind === 'idle' ? 0.35 : s.kind === 'thermal' ? 1.1 : 0.6;
    const jitter = s.kind === 'thermal' ? 0.05 : 0;
    const pinchX = s.kind === 'bottleneck' ? 0.62 : -1;
    const lines = 6;
    const mid = h * 0.56;

    for (let l = lines - 1; l >= 0; l--) {
      const main = l === 0;
      ctx.beginPath();
      const phase = l * 0.9;
      const a = this.amp * h * (1 - l * 0.11);
      for (let i = 0; i <= 160; i++) {
        const xf = i / 160;
        let env = Math.sin(Math.PI * xf) ** 0.8; // fades at the edges
        if (pinchX > 0) env *= 1 - 0.78 * Math.exp(-(((xf - pinchX) / 0.07) ** 2));
        const y =
          mid +
          env *
            a *
            (Math.sin(xf * 7.5 + t * speed * 2 + phase) * 0.55 +
              Math.sin(xf * 15.3 - t * speed * 1.3 + phase * 1.7) * 0.28 +
              Math.sin(xf * 31 + t * speed * 3.1) * 0.08 +
              (jitter ? (Math.sin(xf * 97 + t * 23 + l) * jitter) : 0));
        const x = xf * w;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.lineWidth = main ? 2 : 1;
      ctx.globalAlpha = main ? 1 : 0.5 - l * 0.06;
      ctx.strokeStyle = main ? (s.kind === 'thermal' ? this.colors.warn : s.kind === 'idle' ? this.colors.ink : this.colors.accent) : this.colors.faint;
      ctx.stroke();
    }
    ctx.globalAlpha = 1;

    if (pinchX > 0) {
      // Marker where the flow is constricted.
      const x = pinchX * w;
      ctx.strokeStyle = this.colors.faint;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x, 34);
      ctx.lineTo(x, h - 14);
      ctx.stroke();
    }
  }

  private drawSession(): void {
    const { ctx, w, h } = this;
    const r = this.session?.recent ?? [];
    if (r.length < 2) return;
    const top = 48;
    const bottom = h - 18;
    const max = Math.max(...r) * 1.1;
    const min = Math.min(...r) * 0.9;
    const y = (v: number) => bottom - ((v - min) / (max - min || 1)) * (bottom - top);
    // area wash
    ctx.beginPath();
    r.forEach((v, i) => {
      const x = (i / (r.length - 1)) * w;
      if (i === 0) ctx.moveTo(x, y(v));
      else ctx.lineTo(x, y(v));
    });
    ctx.lineTo(w, bottom);
    ctx.lineTo(0, bottom);
    ctx.closePath();
    ctx.globalAlpha = 0.1;
    ctx.fillStyle = this.colors.accent;
    ctx.fill();
    ctx.globalAlpha = 1;
    // line
    ctx.beginPath();
    r.forEach((v, i) => {
      const x = (i / (r.length - 1)) * w;
      if (i === 0) ctx.moveTo(x, y(v));
      else ctx.lineTo(x, y(v));
    });
    ctx.lineWidth = 2;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    ctx.strokeStyle = this.colors.accent;
    ctx.stroke();
    // end dot with surface ring
    const lx = w - 1;
    const ly = y(r[r.length - 1]);
    ctx.beginPath();
    ctx.arc(lx - 4, ly, 4, 0, Math.PI * 2);
    ctx.fillStyle = this.colors.accent;
    ctx.fill();
  }
}
