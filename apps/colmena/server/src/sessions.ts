import { randomUUID } from "node:crypto";
import { Colmena, type Msg } from "./colmena";
import { AgentSimulator } from "./sim";

export interface Limits {
  maxSessions: number;
  idleMs: number;
  maxDocs: number;
  seedDocs: number;
}

export const LIMITS: Limits = {
  maxSessions: Number(process.env.MAX_SESSIONS ?? 8),
  idleMs: Number(process.env.SESSION_IDLE_MIN ?? 15) * 60_000,
  maxDocs: Number(process.env.MAX_DOCS ?? 1500),
  seedDocs: Number(process.env.SEED_DOCS ?? 240),
};

export class SessionFull extends Error {}

interface Sink {
  send(data: string): unknown;
}

/** Una colmena aislada (DB propia + agentes propios) por visitante. */
export class Session {
  sim: AgentSimulator;
  /** El visitante pausó a los agentes: no se reanudan al reconectar. */
  userPaused = false;
  readonly sockets = new Set<Sink>();
  lastSeen = Date.now();
  private off!: () => void;
  private pauseTimer?: ReturnType<typeof setTimeout>;

  constructor(
    readonly id: string,
    public colmena: Colmena
  ) {
    this.sim = new AgentSimulator(colmena);
    this.wire();
  }

  private wire() {
    this.off = this.colmena.on((m: Msg) => {
      const s = JSON.stringify(m);
      for (const ws of this.sockets) ws.send(s);
    });
  }

  setRunning(on: boolean) {
    this.userPaused = !on;
    if (on) this.sim.start();
    else this.sim.stop();
  }

  /** Reinicia la colmena de esta sesión: vacía (`seedDocs = 0`) o con los datos de ejemplo. */
  async reset(seedDocs: number) {
    const speed = this.sim.speed;
    this.sim.stop();
    this.off();
    const old = this.colmena;
    this.colmena = await Colmena.create(seedDocs);
    old.close();
    this.sim = new AgentSimulator(this.colmena);
    this.sim.speed = speed;
    this.wire();
    for (const ws of this.sockets) ws.send(JSON.stringify({ t: "reset" } satisfies Msg));
    if (!this.userPaused && this.sockets.size) this.sim.start();
  }

  touch() {
    this.lastSeen = Date.now();
  }

  attach(ws: Sink) {
    this.sockets.add(ws);
    clearTimeout(this.pauseTimer);
    if (!this.userPaused) this.sim.start();
    this.touch();
    ws.send(JSON.stringify({ t: "stats", stats: this.colmena.stats() }));
  }

  detach(ws: Sink) {
    this.sockets.delete(ws);
    // Sin pestañas abiertas los agentes se pausan (tras una gracia para recargas).
    if (this.sockets.size === 0) {
      this.pauseTimer = setTimeout(() => this.sim.stop(), 5_000);
    }
  }

  dispose() {
    clearTimeout(this.pauseTimer);
    this.sim.stop();
    this.off();
    for (const ws of this.sockets) (ws as { close?: () => void }).close?.();
    this.sockets.clear();
    this.colmena.close();
  }
}

export class SessionManager {
  private sessions = new Map<string, Session>();
  private sweeper = setInterval(() => this.sweep(), 30_000);

  constructor(private limits: Limits = LIMITS) {}

  get size() {
    return this.sessions.size;
  }

  async create(): Promise<Session> {
    this.sweep();
    if (this.sessions.size >= this.limits.maxSessions) throw new SessionFull();
    const colmena = await Colmena.create(this.limits.seedDocs);
    const s = new Session(randomUUID(), colmena);
    this.sessions.set(s.id, s);
    return s;
  }

  get(id: string | null | undefined): Session | undefined {
    const s = id ? this.sessions.get(id) : undefined;
    s?.touch();
    return s;
  }

  destroy(id: string) {
    const s = this.sessions.get(id);
    if (!s) return false;
    this.sessions.delete(id);
    s.dispose();
    return true;
  }

  private sweep() {
    const cutoff = Date.now() - this.limits.idleMs;
    for (const [id, s] of this.sessions) {
      // Una pestaña abierta mantiene viva la sesión.
      if (s.sockets.size === 0 && s.lastSeen < cutoff) this.destroy(id);
    }
  }

  closeAll() {
    clearInterval(this.sweeper);
    for (const id of [...this.sessions.keys()]) this.destroy(id);
  }
}

/** Ventana deslizante simple por clave (IP). */
export class RateLimiter {
  private hits = new Map<string, number[]>();
  constructor(
    private max: number,
    private windowMs: number
  ) {}

  allow(key: string): boolean {
    const now = Date.now();
    const arr = (this.hits.get(key) ?? []).filter((t) => now - t < this.windowMs);
    if (arr.length >= this.max) {
      this.hits.set(key, arr);
      return false;
    }
    arr.push(now);
    this.hits.set(key, arr);
    if (this.hits.size > 5000) for (const [k, v] of this.hits) if (!v.some((t) => now - t < this.windowMs)) this.hits.delete(k);
    return true;
  }
}
