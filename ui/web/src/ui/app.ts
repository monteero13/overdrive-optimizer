// App shell: wires EngineClient → regions. The UI is a function of
// (EngineSnapshot, UiState). No region invents system state.
import { RequestError, type EngineClient } from '../engine/client.ts';
import { setLocale, t } from '../i18n/index.ts';
import type { MockService } from '../mock/service.ts';
import type { PresetId } from '../mock/presets.ts';
import type { EngineSnapshot, PlanPreview, SessionReport, Settings } from '../proto/types.ts';
import { patch } from './dom.ts';
import { bindMagnetic } from './magnetic.ts';
import { banner, coachModal, confirmModal, devPanel, hud, opModal, promptsAndToasts, settingsModal, topbar, tray } from './overlays.ts';
import { Pulse } from './pulse.ts';
import { reportView, wireReportChart } from './report.ts';
import { initialUi, type UiState } from './state.ts';
import { gamesView, historyView, pcView } from './views.ts';

const motionQuery = matchMedia('(prefers-reduced-motion: reduce)');

export class App {
  private ui: UiState = initialUi();
  private pulse = new Pulse();
  private regions: Record<'top' | 'main' | 'over' | 'hud' | 'dev', HTMLElement>;
  private toastSeq = 0;
  private ticker: ReturnType<typeof setInterval> | null = null;
  private thermal = false;
  private presetId: PresetId = 'laptop_hybrid';
  private lastTheme = '';
  private client: EngineClient;
  private mock: MockService | null;

  constructor(root: HTMLElement, client: EngineClient, mock: MockService | null) {
    this.client = client;
    this.mock = mock;
    root.classList.toggle('has-dev', !!mock);
    root.innerHTML = `<div data-r="top"></div><main class="main" data-r="main" id="main"></main><div data-r="hud"></div><div data-r="over"></div><div data-r="dev"></div>`;
    const q = (n: string) => root.querySelector<HTMLElement>(`[data-r="${n}"]`)!;
    this.regions = { top: q('top'), main: q('main'), over: q('over'), hud: q('hud'), dev: q('dev') };
    root.addEventListener('click', (e) => this.onClick(e));
    root.addEventListener('change', (e) => this.onChange(e));
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') this.closeTopmost();
    });
    motionQuery.addEventListener('change', () => this.render());
    matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => this.pulse.refreshColors());
    bindMagnetic(root, () => this.reducedMotion());
    this.pulse.canvas.addEventListener('pointermove', (e) => this.pulseHover(e));
    this.pulse.canvas.addEventListener('pointerleave', () => this.pulseTip(null));

    client.on((ev) => {
      if (ev.type === 'state') this.onSnapshot(ev.snapshot);
      else if (ev.type === 'link') this.render();
      else if (ev.type === 'op.finished') {
        if (ev.outcome === 'done') {
          if (this.ui.planOpen) void this.loadPlan();
          return;
        }
        const key = ev.outcome === 'cancelled' ? 'outcome.cancelled' : (ev.error?.messageKey ?? 'err.internal');
        this.toast(ev.outcome === 'cancelled' ? 'warn' : 'bad', t(key));
      } else if (ev.type === 'session.report') void this.openReport(ev.txId);
    });
    this.render();
  }

  private get snap(): EngineSnapshot | null {
    return this.client.snapshot;
  }

  private reducedMotion(): boolean {
    const m = this.snap?.settings.reduceMotion ?? 'auto';
    return m === 'on' || (m === 'auto' && motionQuery.matches);
  }

  private onSnapshot(s: EngineSnapshot): void {
    setLocale(s.settings.locale);
    if (!this.ui.selected || !s.games.some((g) => g.id === this.ui.selected)) this.ui.selected = s.games[0]?.id ?? null;
    if (s.activeSession && this.ui.view === 'games') this.ui.selected = s.activeSession.gameId;
    if (this.ui.coachOpen && !s.coach) this.ui.coachOpen = false;
    // Elapsed-time ticker only while operations exist.
    if (s.operations.length && !this.ticker) this.ticker = setInterval(() => this.renderOverlays(), 1000);
    if (!s.operations.length && this.ticker) {
      clearInterval(this.ticker);
      this.ticker = null;
    }
    this.render();
  }

  // ------------------------------------------------------------ render
  private applyTheme(s: Settings | undefined): void {
    const root = document.documentElement;
    const theme = s?.theme ?? 'auto';
    if (theme === 'auto') delete root.dataset.theme;
    else root.dataset.theme = theme;
    if (this.reducedMotion()) root.dataset.motion = 'reduced';
    else delete root.dataset.motion;
    if (theme !== this.lastTheme) {
      this.lastTheme = theme;
      this.pulse.refreshColors();
    }
  }

  render(): void {
    const s = this.snap;
    this.applyTheme(s?.settings);
    patch(this.regions.top, topbar(s, this.ui));
    if (s) {
      const html = this.ui.view === 'games' ? gamesView(s, this.ui) : this.ui.view === 'pc' ? pcView(s) : historyView(s);
      patch(this.regions.main, html);
      const slot = this.regions.main.querySelector('[data-slot="pulse"]');
      if (slot && this.pulse.canvas.parentElement !== slot) slot.prepend(this.pulse.canvas);
      this.pulse.set(s.pulse, s.activeSession, this.reducedMotion());
      // The HUD is the "mini-player" for a running session: shown when you are NOT already
      // looking at that game's hero (which has its own live readout).
      const watching = this.ui.view === 'games' && this.ui.selected === s.activeSession?.gameId;
      patch(this.regions.hud, watching ? '' : hud(s));
    } else {
      patch(this.regions.main, `<div class="card pad" style="display:flex;gap:12px;align-items:center"><span class="spinner"></span>${t('link.down')}</div>`);
    }
    this.renderOverlays();
    if (this.mock)
      patch(this.regions.dev, devPanel(s, this.ui, this.mock.scenario, this.presetId, this.client.resyncs, this.thermal) + tray(s, this.ui));
  }

  private renderOverlays(): void {
    const s = this.snap;
    const parts = [
      banner(this.client.link, !!s),
      s && this.ui.report ? reportView(s, this.ui.report, this.ui.reportTable) : '',
      coachModal(s, this.ui),
      settingsModal(s, this.ui),
      confirmModal(this.ui),
      opModal(s),
      promptsAndToasts(s, this.ui),
    ];
    patch(this.regions.over, parts.join(''));
    if (this.ui.report && !this.ui.reportTable) wireReportChart(this.regions.over, this.ui.report);
  }

  private pulseHover(e: PointerEvent): void {
    const s = this.snap;
    const rect = this.pulse.canvas.getBoundingClientRect();
    const xf = (e.clientX - rect.left) / rect.width;
    const v = this.pulse.valueAt(xf);
    if (v !== null) return this.pulseTip({ x: e.clientX - rect.left, y: e.clientY - rect.top, text: `${v.toFixed(1)} ms · ${Math.round(1000 / v)} FPS` });
    if (s?.pulse.kind === 'bottleneck') {
      const g = s.games.find((x) => x.id === this.ui.selected);
      return this.pulseTip({ x: e.clientX - rect.left, y: e.clientY - rect.top, text: `${t(`bottleneck.${s.pulse.label}`)} · ${Math.round(s.pulse.intensity * 100)} %${g ? ` · ${g.fixesReady} fixes` : ''}` });
    }
    if (s?.pulse.kind === 'thermal') return this.pulseTip({ x: e.clientX - rect.left, y: e.clientY - rect.top, text: `Perf. limit ${Math.round(s.pulse.pressure * 100)} %` });
    this.pulseTip(null);
  }

  private pulseTip(p: { x: number; y: number; text: string } | null): void {
    const tip = this.regions.main.querySelector<HTMLElement>('[data-pulse-tip]');
    if (!tip) return;
    if (!p) return tip.classList.remove('on');
    tip.textContent = p.text;
    tip.style.left = `${p.x}px`;
    tip.style.top = `${p.y}px`;
    tip.classList.add('on');
  }

  // ------------------------------------------------------------ helpers
  private toast(tone: 'good' | 'warn' | 'bad', text: string): void {
    const id = ++this.toastSeq;
    this.ui.toasts = [...this.ui.toasts, { id, tone, text }].slice(-3);
    this.renderOverlays();
    setTimeout(() => this.dismissToast(id), 9000);
  }

  private dismissToast(id: number): void {
    this.ui.toasts = this.ui.toasts.filter((x) => x.id !== id);
    this.renderOverlays();
  }

  private async call<T>(method: Parameters<EngineClient['request']>[0], params?: unknown): Promise<T | null> {
    try {
      return await this.client.request<T>(method, params);
    } catch (e) {
      this.toast('bad', t(e instanceof RequestError ? e.body.messageKey : 'err.internal'));
      return null;
    }
  }

  private async loadPlan(): Promise<void> {
    const id = this.ui.selected;
    if (!id) return;
    const plan = await this.call<PlanPreview>('plan.preview', { gameId: id });
    if (plan && plan.gameId === this.ui.selected) {
      this.ui.plan = plan;
      this.render();
    }
  }

  private async openReport(txId: number): Promise<void> {
    const r = await this.call<SessionReport>('report.session', { txId });
    if (!r) return;
    this.ui.report = r;
    this.ui.reportTable = false;
    this.renderOverlays();
    this.regions.over.querySelector<HTMLElement>('[data-key="close-report"]')?.focus();
  }

  private closeTopmost(): void {
    if (this.ui.confirm) this.ui.confirm = null;
    else if (this.ui.report) this.ui.report = null;
    else if (this.ui.coachOpen) this.ui.coachOpen = false;
    else if (this.ui.settingsOpen) this.ui.settingsOpen = false;
    else if (this.ui.trayOpen) this.ui.trayOpen = false;
    else return;
    this.render();
  }

  // ------------------------------------------------------------ actions
  private onChange(e: Event): void {
    const el = e.target as HTMLInputElement | HTMLSelectElement;
    const action = el.dataset.action;
    if (!action || !this.mock) return;
    const checked = (el as HTMLInputElement).checked;
    if (action === 'dev-slow') this.mock.scenario.slow = checked;
    else if (action === 'dev-fail') this.mock.scenario.failNext = checked;
    else if (action === 'dev-thermal') this.mock.setThermal((this.thermal = checked));
    else if (action === 'dev-battery') this.mock.setBattery(checked);
    else if (action === 'dev-preset') {
      this.presetId = el.value as PresetId;
      this.thermal = false;
      this.ui = { ...initialUi(), devOpen: this.ui.devOpen };
      this.mock.switchPreset(this.presetId);
    }
    this.render();
  }

  private onClick(e: MouseEvent): void {
    const target = e.target as HTMLElement;
    const el = target.closest<HTMLElement>('[data-action]');
    if (!el) return;
    const action = el.dataset.action!;
    // Clicking inside a dialog must not trigger its scrim's close action.
    if (action.endsWith('-scrim')) {
      if (target.closest('[data-stop]')) return;
      this.ui[action === 'close-report-scrim' ? 'report' : action === 'close-coach-scrim' ? 'coachOpen' : 'settingsOpen'] = (action === 'close-report-scrim' ? null : false) as never;
      return this.render();
    }
    if (el instanceof HTMLInputElement || el instanceof HTMLSelectElement) return; // handled by change
    const id = el.dataset.id;
    const value = el.dataset.value;
    const s = this.snap;

    switch (action) {
      case 'nav':
        this.ui.view = id as UiState['view'];
        return this.render();
      case 'select-game':
        this.ui.selected = id ?? null;
        this.ui.plan = null;
        this.render();
        if (this.ui.planOpen) void this.loadPlan();
        return;
      case 'toggle-plan':
        this.ui.planOpen = !this.ui.planOpen;
        this.render();
        if (this.ui.planOpen) void this.loadPlan();
        return;
      case 'toggle-tweak':
        void this.call<PlanPreview>('plan.setToggle', { gameId: this.ui.selected, tweakId: id, enabled: el.dataset.enabled === 'true' }).then((p) => {
          if (p) {
            this.ui.plan = p;
            this.render();
          }
        });
        return;
      case 'optimize':
        void this.call('session.optimize', { gameId: this.ui.selected });
        return;
      case 'benchmark':
        void this.call('session.benchmark', { gameId: this.ui.selected });
        return;
      case 'open-report':
        void this.openReport(Number(id));
        return;
      case 'close-report':
        this.ui.report = null;
        return this.render();
      case 'report-table':
        this.ui.reportTable = !this.ui.reportTable;
        return this.renderOverlays();
      case 'rescan':
        void this.call('hardware.rescan');
        return;
      case 'fix-software':
        void this.call('persistent.accept', { tweakId: id });
        return;
      case 'undo-persistent':
        void this.call('persistent.revert', { tweakId: id });
        return;
      case 'revert-all':
        this.ui.trayOpen = false;
        this.ui.confirm = { text: t('pc.revert_all.confirm'), action: 'tx.revertAll' };
        return this.render();
      case 'coach-reboot':
        this.ui.confirm = { text: t('coach.reboot_confirm'), action: 'bios.rebootToFirmware' };
        return this.render();
      case 'confirm-yes': {
        const c = this.ui.confirm;
        this.ui.confirm = null;
        if (c?.action === 'tx.revertAll') void this.call('tx.revertAll');
        if (c?.action === 'bios.rebootToFirmware') {
          this.ui.coachOpen = false;
          void this.call('bios.rebootToFirmware', { confirm: true });
        }
        return this.render();
      }
      case 'confirm-no':
        this.ui.confirm = null;
        return this.render();
      case 'open-coach':
        void this.call('bios.startFlow', { issueId: id }).then((r) => {
          if (r) {
            this.ui.coachOpen = true;
            this.render();
          }
        });
        return;
      case 'close-coach':
        this.ui.coachOpen = false;
        return this.render();
      case 'resume-all':
        void this.call('background.resumeAll');
        return;
      case 'answer':
        void this.call('prompt.answer', { promptId: id, choice: value });
        return;
      case 'dismiss-toast':
        return this.dismissToast(Number(id));
      case 'cancel-op':
        void this.call('op.cancel', { opId: id });
        return;
      case 'open-settings':
        this.ui.settingsOpen = true;
        this.render();
        this.regions.over.querySelector<HTMLElement>('[data-key="close-settings"]')?.focus();
        return;
      case 'close-settings':
        this.ui.settingsOpen = false;
        return this.render();
      case 'set': {
        if (!id || !s) return;
        const v: unknown = value === 'true' ? true : value === 'false' ? false : value;
        void this.call('settings.set', { [id]: v }).then(() => {
          if (id === 'mode' && this.ui.planOpen) void this.loadPlan();
        });
        return;
      }
      case 'tray-open':
        this.ui.trayOpen = false;
        this.ui.view = 'games';
        return this.render();
      // ---- dev (mock only) ----
      case 'dev-toggle':
        e.preventDefault();
        this.ui.devOpen = !this.ui.devOpen;
        return this.render();
      case 'dev-launch':
        if (this.ui.selected) this.mock?.launchGame(this.ui.selected);
        this.ui.view = 'games';
        return this.render();
      case 'dev-close':
        this.mock?.closeGame();
        return;
      case 'dev-crash':
        this.mock?.crash();
        return;
      case 'dev-gap':
        this.mock?.gapNext();
        return;
      case 'dev-drift':
        this.mock?.drift(value as 'windows_update' | 'driver_update');
        return;
      case 'dev-tray':
        this.ui.trayOpen = !this.ui.trayOpen;
        return this.render();
    }
  }
}
