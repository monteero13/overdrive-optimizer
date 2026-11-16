// Real transport: frames go through src-tauri (a relay) to \.\pipe\overdrive.v1.
// The WebView never talks to the pipe or the OS directly; it can only call the
// whitelisted `ipc_send` command and listen to two events.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { LinkStatus, Transport } from './transport.ts';

export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export class TauriTransport implements Transport {
  private frameCbs: ((f: string) => void)[] = [];
  private statusCbs: ((s: LinkStatus) => void)[] = [];
  private up = false;

  async start(): Promise<void> {
    await listen<string>('ipc://frame', (e) => this.frameCbs.forEach((cb) => cb(e.payload)));
    await listen<boolean>('ipc://status', (e) => this.set(e.payload));
    // The relay may have connected before we subscribed: ask for the current state.
    this.set(await invoke<boolean>('ipc_status'));
  }

  private set(connected: boolean): void {
    if (connected === this.up) return;
    this.up = connected;
    const s: LinkStatus = connected ? 'connected' : 'disconnected';
    this.statusCbs.forEach((cb) => cb(s));
  }

  send(frame: string): void {
    if (!this.up) return;
    invoke('ipc_send', { frame }).catch(() => this.set(false));
  }

  onFrame(cb: (frame: string) => void): void {
    this.frameCbs.push(cb);
  }

  onStatus(cb: (s: LinkStatus) => void): void {
    this.statusCbs.push(cb);
  }
}
