import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HiveDB, renderTaskState, type StoredTaskState } from "@johpaz/hive-db";
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

/** Lo que ve el laboratorio del relevo entre contextos. */
export interface RelayView {
  state: StoredTaskState;
  /** El estado como lo recibiría el modelo en su prompt. */
  rendered: string;
  /** Caracteres del historial completo que el relevo reemplaza, y del relevo. */
  fullChars: number;
  relayChars: number;
}

export interface ResumeView extends RelayView {
  /** Decisiones y llamadas del hilo causal que `buildAgentContext` trajo a la ventana. */
  items: { type: string; text: string }[];
  /** Episodios de trabajo parecido (`episodicSimilarity`), ya con su texto. */
  episodes: { id: string; text: string }[];
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
  | { t: "stats"; stats: Stats }
  | { t: "reset" };

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
export const OWN_TOPIC = "propio";
export type QueryMode = "hybrid" | "text" | "vector";

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
    // La base PCA se ajusta siempre con un corpus de referencia, aunque la colmena arranque vacía.
    const fitN = n === 0 ? 240 : n;
    for (let i = 0; i < fitN; i++) {
      const agent = AGENTS[i % AGENTS.length]!;
      const topicId = pick(r, agent.topics);
      const topic = TOPICS.find((t) => t.id === topicId)!;
      const text = makeFact(r, topic);
      staged.push({ id: `m${++this.counter}`, agent, topic: topic.id, text, vec: embed(text) });
    }
    this.pca.fit(staged.map((s) => s.vec));
    staged.length = n;
    this.counter = n;
    if (n === 0) return;
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
      topics: [...TOPICS.map(({ id, label, color }) => ({ id, label, color })), { id: OWN_TOPIC, label: "Tus recuerdos", color: "#ffffff" }],
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

  async remember(agent: string, text: string, causation?: number, maxDocs = Infinity): Promise<DocRec> {
    if (this.docs.size >= maxDocs) throw new Error("LIMIT: la colmena alcanzó su tope de recuerdos");
    const vec = embed(text);
    const id = `m${++this.counter}`;
    const topic = agent === "tú" ? OWN_TOPIC : (topicOf(text) ?? OWN_TOPIC);
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

  async recall(agent: string, text: string, opts: { k?: number; crossAgent?: boolean; efSearch?: number; mode?: QueryMode; only?: string } = {}) {
    const k = opts.k ?? 5;
    const vec = embed(text);
    const mode = opts.mode ?? "hybrid";
    const filters = opts.only
      ? [{ field: "agent", value: opts.only }]
      : opts.crossAgent
        ? undefined
        : [{ field: "agent", value: agent }];
    const efSearch = opts.efSearch;
    const t0 = performance.now();
    const fused = await this.db.queryHybrid({
      text: mode === "vector" ? undefined : text,
      vector: mode === "text" ? undefined : vec,
      k,
      filters,
      efSearch,
    });
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
      filtered: !!filters,
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

  /**
   * Cierra un contexto: destila lo que sabe `agent` en un estado de seis campos (como pide «You don't need
   * long context», arXiv 2611.05306), lo guarda con `saveTaskState` y lo indexa como episodio para que
   * otra tarea parecida lo encuentre. Sin modelo: lo vivo son los hechos, lo olvidado son los callejones
   * sin salida. La siguiente instancia arranca de este estado y no del historial.
   */
  async handoff(agent: string, objective: string): Promise<RelayView> {
    const mine = [...this.docs.values()].filter((d) => d.agent === agent).sort((a, b) => a.seq - b.seq);
    const alive = mine.filter((d) => d.alive);
    const dropped = mine.filter((d) => !d.alive);
    const taskId = `task-${agent}`;
    const previous = await this.db.loadTaskState(agent, taskId);
    const state = await this.db.saveTaskState(
      agent,
      taskId,
      {
        objective,
        facts: alive.slice(-8).map((d) => d.text),
        changes: [`${alive.length} recuerdo(s) vivos en la memoria de ${agent}`],
        verification: [`${this.eventCount} evento(s) en el log de la sesión`],
        deadEnds: dropped.slice(-5).map((d) => `Olvidado: ${d.text}`),
        plan: ["Continuar desde los hechos de arriba sin repetir lo olvidado"],
      },
      previous ? { expectedVersion: previous.version } : { expectedVersion: 0 },
    );
    const body = `${objective}. ${state.facts.join(" ")}`;
    const epId = `ep:${agent}:${state.step}`;
    await this.db.upsertDoc({
      id: epId,
      body,
      vector: embed(body),
      filters: [
        { field: "kind", value: "episode" },
        { field: "agent", value: agent },
      ],
    });
    this.episodes.set(epId, body);
    await this.logEvent(agent, "ToolCall", { tool: "task_state.save", step: state.step }, `relevo · paso ${state.step}`);
    return this.relayView(state, mine);
  }

  /**
   * Arranca un contexto nuevo: la instancia siguiente no recibe el historial, solo el estado guardado más
   * la ventana causal de `buildAgentContext`, que además trae episodios parecidos (`episodicSimilarity`).
   */
  async resume(agent: string, objective: string): Promise<ResumeView | undefined> {
    const state = await this.db.loadTaskState(agent, `task-${agent}`);
    if (!state) return undefined;
    const ctx = (await this.db.buildAgentContext({
      taskId: `task-${agent}`,
      currentPhase: "execute",
      currentObjective: objective,
      maxTokens: 800,
      strategy: {
        causalAnchors: true,
        episodicSimilarity: { vector: embed(objective), k: 4, filters: [{ field: "agent", value: agent }] },
      },
      agents: [agent],
    })) as { items?: { type: string; text?: string }[]; similarEpisodes?: { taskId: string }[] };
    // El motor devuelve solo el id del episodio; el texto lo guardamos nosotros. Se descarta el propio
    // estado actual y, por si el binario es anterior al filtro, todo lo que no sea de este agente.
    const own = `ep:${agent}:`;
    const episodes = (ctx.similarEpisodes ?? [])
      .filter((e) => e.taskId.startsWith(own) && e.taskId !== `${own}${state.step}`)
      .map((e) => ({ id: e.taskId, text: this.episodes.get(e.taskId) ?? "" }))
      .filter((e) => e.text);
    const items = (ctx.items ?? [])
      .filter((i) => i.type !== "episode" && i.text)
      .map((i) => ({ type: i.type, text: i.text! }));
    const mine = [...this.docs.values()].filter((d) => d.agent === agent);
    return { ...this.relayView(state, mine), items, episodes };
  }

  private episodes = new Map<string, string>();

  private relayView(state: StoredTaskState, mine: DocRec[]): RelayView {
    const rendered = renderTaskState(state);
    return { state, rendered, fullChars: mine.reduce((n, d) => n + d.text.length, 0), relayChars: rendered.length };
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
