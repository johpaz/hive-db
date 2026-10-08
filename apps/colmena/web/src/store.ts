import { create } from "zustand";
import { api, ApiError, ensureSession, currentSid } from "./api";
import { addBeam, addRing, agentFlash, agentPos, COLORS, hex, now } from "./fx";
import type { AgentInfo, DocRec, EventMsg, Msg, QueryMsg, Stats, TopicInfo } from "./types";

interface FeedItem {
  id: number;
  kind: "event" | "query" | "gate" | "invalidate";
  text: string;
  agent: string;
  ok?: boolean;
  ts: number;
}

interface State {
  connected: boolean;
  error: string | null;
  agents: AgentInfo[];
  topics: TopicInfo[];
  docs: DocRec[];
  docIndex: Map<string, number>;
  docsVersion: number;
  stats: Stats | null;
  events: EventMsg[];
  feed: FeedItem[];
  lastQuery: QueryMsg | null;
  running: boolean;
  lab: boolean;
  setLab(on: boolean): void;
  reload(): Promise<void>;
  selected: string | null;
  selectedEvent: number | null;
  tour: number | null;
  setTour(step: number | null): void;
  selectEvent(seq: number | null): void;
  highlight: Map<string, number>;
  init(): void;
  select(id: string | null): void;
}

let feedId = 0;
let started = false;

export const useStore = create<State>((set, get) => ({
  connected: false,
  error: null,
  agents: [],
  topics: [],
  docs: [],
  docIndex: new Map(),
  docsVersion: 0,
  stats: null,
  events: [],
  feed: [],
  lastQuery: null,
  running: true,
  lab: false,
  setLab: (lab) => set(lab ? { lab, tour: null } : { lab }),
  async reload() {
    const snap = await api<Snapshot>("/snapshot");
    applySnapshot(snap);
  },
  selected: null,
  selectedEvent: null,
  tour: null,
  setTour: (step) => set({ tour: step }),
  selectEvent: (seq) => set({ selectedEvent: seq }),
  highlight: new Map(),
  select: (id) => set({ selected: id }),
  init() {
    if (started) return;
    started = true;
    (async () => {
      try {
        applySnapshot(await api<Snapshot>("/snapshot"));
        connect();
      } catch (e) {
        started = false;
        const full = e instanceof ApiError && (e.status === 503 || e.status === 429);
        set({ error: e instanceof Error ? e.message : String(e) });
        if (full) setTimeout(() => get().init(), 15_000);
      }
    })();
  },
}));

type Snapshot = { agents: AgentInfo[]; topics: TopicInfo[]; docs: DocRec[]; stats: Stats; running: boolean };

function applySnapshot(snap: Snapshot) {
  const docIndex = new Map<string, number>();
  snap.docs.forEach((d, i) => docIndex.set(d.id, i));
  useStore.setState((s) => ({
    agents: snap.agents,
    topics: snap.topics,
    docs: snap.docs,
    docIndex,
    docsVersion: s.docsVersion + 1,
    stats: snap.stats,
    running: snap.running,
    error: null,
  }));
}

function pushFeed(item: Omit<FeedItem, "id" | "ts">) {
  const feed = [{ ...item, id: ++feedId, ts: Date.now() }, ...useStore.getState().feed].slice(0, 40);
  useStore.setState({ feed });
}

function agentIdx(id: string) {
  const { agents } = useStore.getState();
  const i = agents.findIndex((a) => a.id === id);
  return { i: Math.max(0, i), n: Math.max(1, agents.length), color: hex(agents[i]?.color ?? "#ffffff") };
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/live?sid=${currentSid()}`);
  ws.onopen = () => useStore.setState({ connected: true });
  ws.onclose = async () => {
    useStore.setState({ connected: false });
    // Si la sesión expiró, el servidor cerró el socket: pide otra y recarga el estado.
    try {
      const r = await fetch("/api/snapshot", { headers: { "x-sid": currentSid() ?? "" } });
      if (r.status === 401) {
        await ensureSession(true);
        started = false;
        useStore.setState({ docs: [], docIndex: new Map(), events: [], feed: [], lastQuery: null, selected: null, selectedEvent: null });
        useStore.getState().init();
        return;
      }
    } catch {}
    setTimeout(connect, 1500);
  };
  ws.onmessage = (e) => handle(JSON.parse(e.data) as Msg);
}

function handle(m: Msg) {
  const s = useStore.getState();
  const t = now();
  switch (m.t) {
    case "stats":
      useStore.setState({ stats: m.stats });
      break;
    case "reset":
      useStore.setState({ events: [], feed: [], lastQuery: null, selected: null, selectedEvent: null, highlight: new Map() });
      void useStore.getState().reload();
      break;
    case "doc": {
      if (s.docIndex.has(m.doc.id)) break;
      const docs = [...s.docs, m.doc];
      const docIndex = new Map(s.docIndex);
      docIndex.set(m.doc.id, docs.length - 1);
      useStore.setState({ docs, docIndex, docsVersion: s.docsVersion + 1 });
      const a = agentIdx(m.doc.agent);
      const from = agentPos(a.i, a.n, t);
      addBeam({ from, to: m.doc.xyz, color: a.color, t0: t, dur: 0.9 });
      addRing({ at: m.doc.xyz, color: a.color, t0: t + 0.7, dur: 1.2, maxR: 1.8 });
      agentFlash.set(m.doc.agent, { t0: t, color: a.color });
      break;
    }
    case "event": {
      const events = [m, ...s.events].slice(0, 90);
      useStore.setState({ events });
      pushFeed({ kind: "event", agent: m.agent, text: `${m.kind} · ${m.label}` });
      break;
    }
    case "invalidate": {
      const idx = s.docIndex.get(m.id);
      if (idx !== undefined) {
        const docs = s.docs.slice();
        docs[idx] = { ...docs[idx]!, alive: false, t: Date.now() };
        useStore.setState({ docs, docsVersion: s.docsVersion + 1 });
        addRing({ at: docs[idx]!.xyz, color: COLORS.dead, t0: t, dur: 1.4, maxR: 2.5 });
      }
      pushFeed({ kind: "invalidate", agent: m.agent, text: `olvidó ${m.id} (seq ${m.seq})` });
      break;
    }
    case "gate": {
      const a = agentIdx(m.agent);
      agentFlash.set(m.agent, { t0: t, color: m.allowed ? COLORS.gateOk : COLORS.gateNo });
      addRing({ at: agentPos(a.i, a.n, t), color: m.allowed ? COLORS.gateOk : COLORS.gateNo, t0: t, dur: 1.2, maxR: 3 });
      pushFeed({
        kind: "gate",
        agent: m.agent,
        ok: m.allowed,
        text: `${m.action} ${m.resource} → ${m.allowed ? "permitido" : "denegado"}`,
      });
      break;
    }
    case "query":
      playQuery(m);
      break;
  }
}

function playQuery(m: QueryMsg) {
  const t = now();
  const a = agentIdx(m.agent);
  const from = agentPos(a.i, a.n, t);
  agentFlash.set(m.agent, { t0: t, color: a.color });
  // 1) el agente lanza la consulta hacia el punto del espacio vectorial
  addBeam({ from, to: m.origin, color: a.color, t0: t, dur: 0.6, width: 2 });
  addRing({ at: m.origin, color: a.color, t0: t + 0.5, dur: 1.6, maxR: 9 });
  // 2) carril vectorial: la ruta REAL del HNSW (capas altas → capa 0), paso a paso
  const LAYER_TINT: [number, number, number][] = [
    [1.0, 0.69, 0.13],
    [1.0, 0.45, 0.2],
    [1.0, 0.25, 0.5],
    [0.85, 0.3, 1.0],
  ];
  if (m.hnsw && m.hnsw.steps.length) {
    const n = m.hnsw.steps.length;
    const span = 1.0;
    m.hnsw.steps.forEach((st, i) => {
      const c = LAYER_TINT[Math.min(st.layer, LAYER_TINT.length - 1)]!;
      const from = st.from ?? m.origin;
      addBeam({
        from,
        to: st.to,
        color: st.layer > 0 ? c : [c[0] * 0.55, c[1] * 0.55, c[2] * 0.55],
        t0: t + 0.7 + (i / n) * span,
        dur: 0.35,
        width: st.layer > 0 ? 2 : 1,
      });
    });
  } else {
    m.vector_hits.forEach((h, i) => {
      if (h.xyz) addBeam({ from: m.origin, to: h.xyz, color: COLORS.vector, t0: t + 0.7 + i * 0.05, dur: 0.7 });
    });
  }
  // 3) carril BM25 (términos → documentos)
  m.text_hits.forEach((h, i) => {
    if (h.xyz) addBeam({ from: m.origin, to: h.xyz, color: COLORS.text, t0: t + 1.1 + i * 0.05, dur: 0.7 });
  });
  // 4) fusión RRF: convergen en los top-k
  const hl = new Map(useStore.getState().highlight);
  m.fused.forEach((h, i) => {
    if (!h.xyz) return;
    addBeam({ from: m.origin, to: h.xyz, color: COLORS.fused, t0: t + 2.1 + i * 0.12, dur: 0.6, width: 2 });
    addRing({ at: h.xyz, color: COLORS.fused, t0: t + 2.5 + i * 0.12, dur: 1.2, maxR: 2.2 });
    hl.set(h.id, t + 2.3 + i * 0.12);
  });
  useStore.setState({ highlight: hl, ...(m.agent === "tú" ? { lastQuery: m } : {}) });
  pushFeed({
    kind: "query",
    agent: m.agent,
    text: `"${m.text}" → ${m.fused.length} hits · ${m.latencyMs.toFixed(2)} ms${m.filtered ? "" : " · multi-agente"}`,
  });
}
