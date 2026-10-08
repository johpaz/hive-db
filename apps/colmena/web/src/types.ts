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

export interface AgentInfo {
  id: string;
  label: string;
  color: string;
  topics: string[];
}

export interface TopicInfo {
  id: string;
  label: string;
  color: string;
}

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

export interface QueryMsg {
  t: "query";
  agent: string;
  text: string;
  origin: XYZ;
  latencyMs: number;
  text_hits: HitRec[];
  vector_hits: HitRec[];
  fused: HitRec[];
  filtered: boolean;
  hnsw: { steps: { layer: number; from?: XYZ; to: XYZ; id: string; distance: number }[]; total: number; layers: number } | null;
  terms: string[];
}

export interface EventMsg {
  t: "event";
  seq: number;
  agent: string;
  kind: string;
  ts: number;
  causation?: number;
  docId?: string;
  label: string;
}

export type Msg =
  | { t: "doc"; doc: DocRec }
  | EventMsg
  | QueryMsg
  | { t: "invalidate"; id: string; seq: number; agent: string }
  | { t: "gate"; agent: string; action: string; resource: string; allowed: boolean }
  | { t: "stats"; stats: Stats }
  | { t: "reset" };
