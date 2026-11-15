import '@fontsource-variable/inter';
import '@fontsource/jetbrains-mono/500.css';
import '@fontsource/jetbrains-mono/600.css';
import './styles/app.css';

import { EngineClient } from './engine/client.ts';
import { MockTransport } from './engine/transport.ts';
import { MockService } from './mock/service.ts';
import { App } from './ui/app.ts';

// Phase 2: always the mock engine. Phase 3+: a TauriTransport relays to \\.\pipe\overdrive.v1
// and the dev panel disappears (mock === null).
const service = new MockService('laptop_hybrid');
const transport = new MockTransport(service);
service.link = { down: () => transport.disconnect(), up: () => transport.connect() };

const client = new EngineClient(transport);
new App(document.getElementById('app')!, client, service);
transport.connect();

// Dev-only deep links to reach states quickly (screenshots, reviews):
//   ?preset=desktop_entry&mode=expert&theme=light&locale=en&view=pc&plan=1&session=1&report=1&dev=0
if (import.meta.env.DEV) void demo(new URLSearchParams(location.search));

async function demo(q: URLSearchParams): Promise<void> {
  if (![...q.keys()].length) return;
  const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const click = (sel: string) => document.querySelector<HTMLElement>(sel)?.click();
  const preset = q.get('preset');
  if (preset === 'desktop_entry' || preset === 'apu_lowend') {
    service.reset(preset);
    await client.resync();
  }
  while (!client.snapshot) await wait(20);
  const settings: Record<string, string> = {};
  for (const k of ['mode', 'theme', 'locale', 'reduceMotion']) if (q.get(k)) settings[k] = q.get(k)!;
  if (Object.keys(settings).length) await client.request('settings.set', settings);
  if (q.get('dev') === '0') click('[data-action="dev-toggle"]');
  if (q.get('game')) click(`[data-action="select-game"][data-id="${q.get('game')}"]`);
  if (q.get('plan')) click('[data-action="toggle-plan"]');
  if (q.get('session') || q.get('report')) {
    const id = client.snapshot.games[0].id;
    const { opId } = await client.request<{ opId: string }>('session.optimize', { gameId: id });
    await new Promise<void>((res) => client.on((e) => e.type === 'op.finished' && e.opId === opId && res()));
    service.launchGame(id);
    await wait(q.get('report') ? 3500 : 6000);
    if (q.get('report')) service.closeGame();
  }
  if (q.get('view')) click(`[data-action="nav"][data-id="${q.get('view')}"]`);
  if (q.get('coach')) {
    const issue = client.snapshot.issues.find((i) => i.fixableBy.kind === 'bios');
    if (issue) click(`[data-action="open-coach"][data-id="${issue.id}"]`);
  }
}
