import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HiveDB } from "@johpaz/hive-db";
import { AGENTS, TOPICS, makeFact, pick, rng, type AgentSpec } from "./dataset";
import { DIM, embed, tokenize, topicOf } from "./embed";
import { Pca3 } from "./pca";

export type XYZ = [number, number, number];

export interface DocRec {
  id: string;
  agent: string;
  topic: string;
  text: string;
  xyz: XYZ;
  seq: number;
  alive: boolean;
  t: number;
}

export interface HitRec {
  id: string;
  score: number;
  textScore?: number;
  vectorScore?: number;
  xyz?: XYZ;
}

export interface TraceStepRec {
  layer: number;
  from?: XYZ;
  to: XYZ;
  id: string;
  distance: number;
}

export type Msg =
  | { t: "doc"; doc: DocRec }
  | { t: "event"; seq: number; agent: string; kind: string; ts: number; causation?: number; docId?: string; label: string }
  | {
      t: "query";
      agent: string;
      text: string;
      origin: XYZ;
      latencyMs: number;
      text_hits: HitRec[];
      vector_hits: HitRec[];
      fused: HitRec[];
      filtered: boolean;
      hnsw: { steps: TraceStepRec[]; total: number; layers: number } | null;
      terms: string[];
    }
  | { t: "invalidate"; id: string; seq: number; agent: string }
  | { t: "gate"; agent: string; action: string; resource: string; allowed: boolean }
  | { t: "stats"; stats: Stats };

export interface Stats {
  docs: number;
  alive: number;
  events: number;
  queries: number;
  inserts: number;
  p50: number;
  p99: number;
  uptimeS: number;
}

const STREAM = "memory";

export class Colmena {
  readonly agents: AgentSpec[] = AGENTS;
  readonly docs = new Map<string, DocRec>();
  private listeners = new Set<(m: Msg) => void>();
  private pca = new Pca3(DIM);
  private latencies: number[] = [];
  private queries = 0;
  private inserts = 0;
  private counter = 0;
  private started = Date.now();
  private lastFactByAgent = new Map<string, number>();

  private constructor(
    readonly db: HiveDB,
    private dir: string
  ) {}

  static async create(seedDocs = 240): Promise<Colmena> {
    const dir = mkdtempSync(join(tmpdir(), "colmena-"));
    const db = await HiveDB.open(dir, { vector: { dimension: DIM, spaceId: "colmena:bow64" } });
    const c = new Colmena(db, dir);
    await c.seed(seedDocs);
    return c;
  }

  on(fn: (m: Msg) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(m: Msg) {
    for (const fn of this.listeners) fn(m);
  }

  private async seed(n: number) {
    const r = rng(7);
    const staged: { id: string; agent: AgentSpec; topic: string; text: string; vec: Float32Array }[] = [];
    for (let i = 0; i < n; i++) {
      const agent = AGENTS[i % AGENTS.length]!;
      const topicId = pick(r, agent.topics);
      const topic = TOPICS.find((t) => t.id === topicId)!;
      const text = makeFact(r, topic);
      staged.push({ id: `m${++this.counter}`, agent, topic: topic.id, text, vec: embed(text) });
    }
    this.pca.fit(staged.map((s) => s.vec));
    await this.db.upsertBatch(
      staged.map((s) => ({
        id: s.id,
        body: s.text,
        tags: s.topic,
        vector: s.vec,
        filters: [{ field: "agent", value: s.agent.id }],
      }))
    );
    for (const s of staged) {
      const seq = await this.db.append({
        agentId: s.agent.id,
        streamId: STREAM,
        kind: "Fact",
        payload: JSON.stringify({ docId: s.id, text: s.text, topic: s.topic }),
      });
      this.lastFactByAgent.set(s.agent.id, seq);
      this.docs.set(s.id, {
        id: s.id,
        agent: s.agent.id,
        topic: s.topic,
        text: s.text,
        xyz: this.pca.project(s.vec),
        seq,
        alive: true,
        t: Date.now(),
      });
    }
  }

  snapshot() {
    return {
      agents: this.agents,
      topics: TOPICS.map(({ id, label, color }) => ({ id, label, color })),
      docs: [...this.docs.values()],
      stats: this.stats(),
    };
  }

  stats(): Stats {
    const lat = [...this.latencies].sort((a, b) => a - b);
    const q = (p: number) => (lat.length ? lat[Math.min(lat.length - 1, Math.floor(p * lat.length))]! : 0);
    let alive = 0;
    for (const d of this.docs.values()) if (d.alive) alive++;
    return {
      docs: this.docs.size,
      alive,
      events: this.eventCount,
      queries: this.queries,
      inserts: this.inserts,
      p50: q(0.5),
      p99: q(0.99),
      uptimeS: Math.round((Date.now() - this.started) / 1000),
    };
  }

  private eventCount = 0;

  private async logEvent(
    agent: string,
    kind: "Fact" | "MemoryInvalidate" | "ToolCall",
    payload: object,
    label: string,
    causation?: number,
    docId?: string
  ): Promise<number> {
    const seq = await this.db.append({
      agentId: agent,
      streamId: STREAM,
      kind,
      payload: JSON.stringify(payload),
      causation,
    });
    this.eventCount++;
    this.emit({ t: "event", seq, agent, kind, ts: Date.now(), causation, docId, label });
    return seq;
  }

  async remember(agent: string, text: string, causation?: number): Promise<DocRec> {
    const vec = embed(text);
    const id = `m${++this.counter}`;
    const topic = topicOf(text) ?? "viajes";
    await this.db.upsertDoc({ id, body: text, tags: topic, vector: vec, filters: [{ field: "agent", value: agent }] });
    const seq = await this.logEvent(agent, "Fact", { docId: id, text, topic }, text, causation ?? this.lastFactByAgent.get(agent), id);
    this.lastFactByAgent.set(agent, seq);
    await this.db.workingSet(agent, "last_memory", { docId: id }, 30_000);
    const doc: DocRec = { id, agent, topic, text, xyz: this.pca.project(vec), seq, alive: true, t: Date.now() };
    this.docs.set(id, doc);
    this.inserts++;
    this.emit({ t: "doc", doc });
    return doc;
  }

  async recall(agent: string, text: string, opts: { k?: number; crossAgent?: boolean; efSearch?: number } = {}) {
    const k = opts.k ?? 5;
    const vec = embed(text);
    const filters = opts.crossAgent ? undefined : [{ field: "agent", value: agent }];
    const efSearch = opts.efSearch;
    const t0 = performance.now();
    const fused = await this.db.queryHybrid({ text, vector: vec, k, filters, efSearch });
    const latencyMs = performance.now() - t0;
    const [textOnly, vecOnly, trace] = await Promise.all([
      this.db.queryHybrid({ text, k: k + 3, filters }),
      this.db.queryHybrid({ vector: vec, k: k + 3, filters, efSearch }),
      this.db.traceVector(vec, k + 3, efSearch).catch(() => null),
    ]);
    const steps: TraceStepRec[] = [];
    if (trace) {
      for (const st of trace.steps) {
        const to = this.docs.get(st.node)?.xyz;
        if (!to) continue;
        steps.push({ layer: st.layer, from: st.from ? this.docs.get(st.from)?.xyz : undefined, to, id: st.node, distance: st.distance });
      }
    }
    const dec = (hs: { id: string; score: number; textScore?: number; vectorScore?: number }[]): HitRec[] =>
      hs.map((h) => ({ ...h, xyz: this.docs.get(h.id)?.xyz }));
    this.latencies.push(latencyMs);
    if (this.latencies.length > 500) this.latencies.shift();
    this.queries++;
    const msg: Msg = {
      t: "query",
      agent,
      text,
      origin: this.pca.project(vec),
      latencyMs,
      text_hits: dec(textOnly),
      vector_hits: dec(vecOnly),
      fused: dec(fused),
      filtered: !opts.crossAgent,
      hnsw: trace
        ? { steps: steps.slice(0, 220), total: trace.steps.length, layers: steps.reduce((m, x) => Math.max(m, x.layer), 0) + 1 }
        : null,
      terms: [...new Set(tokenize(text))],
    };
    await this.logEvent(agent, "ToolCall", { tool: "memory.recall", query: text, hits: fused.map((h) => h.id) }, `recall · ${text}`);
    this.emit(msg);
    return msg;
  }

  async invalidate(docId: string, byAgent?: string) {
    const d = this.docs.get(docId);
    if (!d || !d.alive) return undefined;
    const agent = byAgent ?? d.agent;
    const seq = await this.logEvent(agent, "MemoryInvalidate", { target_seq: d.seq }, `olvidar ${docId}`, d.seq, docId);
    await this.db.deleteDoc(docId);
    d.alive = false;
    this.emit({ t: "invalidate", id: docId, seq, agent });
    return seq;
  }

  async gate(agent: string, action: string, resource: string) {
    const decision = await this.db.can(agent, action, resource);
    this.emit({ t: "gate", agent, action, resource, allowed: decision.allowed });
    return decision.allowed;
  }

  async context(agent: string, objective: string) {
    return this.db.buildAgentContext({
      taskId: `task-${agent}`,
      currentPhase: "execute",
      currentObjective: objective,
      maxTokens: 800,
      strategy: { causalAnchors: true },
      agents: [agent],
    });
  }

  emitStats() {
    this.emit({ t: "stats", stats: this.stats() });
  }

  close() {
    try {
      this.db.close();
    } catch {}
    try {
      rmSync(this.dir, { recursive: true, force: true });
    } catch {}
  }
}
