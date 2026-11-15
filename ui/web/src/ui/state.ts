import type { PlanPreview, SessionReport } from '../proto/types.ts';

export type View = 'games' | 'pc' | 'history';

export interface Toast {
  id: number;
  tone: 'good' | 'warn' | 'bad';
  text: string;
}

/** UI-local state only. Anything about the system lives in EngineSnapshot. */
export interface UiState {
  view: View;
  selected: string | null;
  planOpen: boolean;
  plan: PlanPreview | null;
  report: SessionReport | null;
  reportTable: boolean;
  coachOpen: boolean;
  settingsOpen: boolean;
  trayOpen: boolean;
  devOpen: boolean;
  confirm: { text: string; action: string; id?: string } | null;
  toasts: Toast[];
}

export const initialUi = (): UiState => ({
  view: 'games',
  selected: null,
  planOpen: false,
  plan: null,
  report: null,
  reportTable: false,
  coachOpen: false,
  settingsOpen: false,
  trayOpen: false,
  devOpen: true,
  confirm: null,
  toasts: [],
});
