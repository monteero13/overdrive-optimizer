// EngineClient: the ONLY source of truth for the UI is the snapshot it holds.
// Lesson v0.1.2: the UI never assumes state; it renders what the engine says.
//  - snapshot on connect, merge patches after that
//  - seq gap or new bootId  -> full resync
//  - disconnect             -> operations cleared locally (no stuck overlays),
//                              pending requests rejected, reconnect -> resync

import { applyMergePatch } from './mergePatch.ts';
import type { LinkStatus, Transport } from './transport.ts';
import type {
  EngineSnapshot,
  ErrorBody,
  Evt,
  Frame,
  HelloResult,
  Method,
  OpOutcome,
} from '../proto/types.ts';

export class RequestError extends Error {
  body: ErrorBody;
  constructor(body: ErrorBody) {
    super(body.code);
    this.body = body;
  }
}

export type ClientEvent =
  | { type: 'state'; snapshot: EngineSnapshot }
  | { type: 'link'; status: LinkStatus }
  | { type: 'op.finished'; opId: string; method: Method; outcome: OpOutcome; error?: ErrorBody }
  | { type: 'session.report'; txId: number };

const DISCONNECTED: ErrorBody = { code: 'SERVICE_DISCONNECTED', messageKey: 'err.service_disconnected' };

export class EngineClient {
  snapshot: EngineSnapshot | null = null;
  link: LinkStatus = 'disconnected';
  resyncs = 0;

  private nextId = 1;
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void; method: Method }>();
  private listeners: ((e: ClientEvent) => void)[] = [];
  private syncing = false;
  private transport: Transport;
  private uiVersion: string;

  constructor(transport: Transport, uiVersion = '1.0.0-proto') {
    this.transport = transport;
    this.uiVersion = uiVersion;
    transport.onFrame((f) => this.onFrame(f));
    transport.onStatus((s) => this.onStatus(s));
  }

  on(cb: (e: ClientEvent) => void): () => void {
    this.listeners.push(cb);
    return () => (this.listeners = this.listeners.filter((l) => l !== cb));
  }

  request<T = unknown>(method: Method, params?: unknown): Promise<T> {
    if (this.link !== 'connected') return Promise.reject(new RequestError(DISCONNECTED));
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject, method });
      this.transport.send(JSON.stringify({ kind: 'req', id, method, params }));
    });
  }

  private emit(e: ClientEvent): void {
    for (const l of this.listeners) l(e);
  }

  private async onStatus(s: LinkStatus): Promise<void> {
    this.link = s;
    if (s === 'disconnected') {
      for (const p of this.pending.values()) p.reject(new RequestError(DISCONNECTED));
      this.pending.clear();
      if (this.snapshot) {
        // Operations can't be trusted while the service is gone: drop them so no
        // overlay can stay stuck. The fresh snapshot after reconnect is the truth.
        for (const op of this.snapshot.operations)
          this.emit({ type: 'op.finished', opId: op.opId, method: op.method, outcome: 'failed', error: DISCONNECTED });
        this.snapshot = { ...this.snapshot, operations: [] };
        this.emit({ type: 'state', snapshot: this.snapshot });
      }
      this.emit({ type: 'link', status: s });
      return;
    }
    this.emit({ type: 'link', status: s });
    try {
      await this.request<HelloResult>('hello', { uiVersion: this.uiVersion, locale: this.snapshot?.settings.locale ?? 'es' });
      await this.resync();
    } catch {
      /* link dropped again; the next 'connected' retries */
    }
  }

  async resync(): Promise<void> {
    if (this.syncing) return;
    this.syncing = true;
    try {
      const snap = await this.request<EngineSnapshot>('state.snapshot');
      this.snapshot = snap;
      this.resyncs++;
      this.emit({ type: 'state', snapshot: snap });
    } finally {
      this.syncing = false;
    }
  }

  private onFrame(raw: string): void {
    let f: Frame;
    try {
      f = JSON.parse(raw) as Frame;
    } catch {
      return;
    }
    if (f.kind === 'res') {
      const p = this.pending.get(f.id);
      if (!p) return;
      this.pending.delete(f.id);
      if (f.ok) p.resolve(f.result);
      else p.reject(new RequestError(f.error));
      return;
    }
    if (f.kind === 'evt') this.onEvent(f);
  }

  private onEvent(e: Evt): void {
    const snap = this.snapshot;
    if (!snap || this.syncing) return; // a snapshot is on its way and will include this
    if (e.seq <= snap.seq) return; // stale / duplicate
    if (e.seq !== snap.seq + 1) {
      void this.resync(); // gap: we missed something -> never render a guessed state
      return;
    }
    if (e.type === 'state.patch') {
      if (e.data.bootId && e.data.bootId !== snap.bootId) {
        void this.resync();
        return;
      }
      this.snapshot = { ...applyMergePatch(snap, e.data), seq: e.seq };
      this.emit({ type: 'state', snapshot: this.snapshot });
      return;
    }
    this.snapshot = { ...snap, seq: e.seq };
    if (e.type === 'op.finished') this.emit({ type: 'op.finished', ...e.data });
    else if (e.type === 'session.report') this.emit({ type: 'session.report', txId: e.data.txId });
  }
}
