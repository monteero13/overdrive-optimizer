// Transport abstraction. The real one (Phase 3+) relays frames through src-tauri
// to the named pipe \\.\pipe\overdrive.v1. The mock one delivers frames to an
// in-page MockService. Frames are always JSON strings, so anything that works
// against the mock is guaranteed to be serializable over the real pipe.

export type LinkStatus = 'connected' | 'disconnected';

export interface Transport {
  send(frame: string): void;
  onFrame(cb: (frame: string) => void): void;
  onStatus(cb: (s: LinkStatus) => void): void;
}

export interface ServerEndpoint {
  /** Called by the transport when a client frame arrives. */
  receive(frame: string, reply: (frame: string) => void): void;
  /** The server pushes events through this. */
  attach(push: (frame: string) => void): void;
  detach(): void;
}

/** In-page transport with simulated latency and disconnects. */
export class MockTransport implements Transport {
  private frameCbs: ((f: string) => void)[] = [];
  private statusCbs: ((s: LinkStatus) => void)[] = [];
  private up = false;
  private server: ServerEndpoint;
  private latencyMs: number;

  constructor(server: ServerEndpoint, latencyMs = 12) {
    this.server = server;
    this.latencyMs = latencyMs;
  }

  connect(): void {
    if (this.up) return;
    this.up = true;
    this.server.attach((f) => this.deliver(f));
    this.statusCbs.forEach((cb) => cb('connected'));
  }

  disconnect(): void {
    if (!this.up) return;
    this.up = false;
    this.server.detach();
    this.statusCbs.forEach((cb) => cb('disconnected'));
  }

  get connected(): boolean {
    return this.up;
  }

  send(frame: string): void {
    if (!this.up) return;
    setTimeout(() => {
      if (this.up) this.server.receive(frame, (f) => this.deliver(f));
    }, this.latencyMs);
  }

  onFrame(cb: (frame: string) => void): void {
    this.frameCbs.push(cb);
  }

  onStatus(cb: (s: LinkStatus) => void): void {
    this.statusCbs.push(cb);
  }

  private deliver(frame: string): void {
    setTimeout(() => {
      if (this.up) this.frameCbs.forEach((cb) => cb(frame));
    }, this.latencyMs);
  }
}
