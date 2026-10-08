import { TOPICS } from "./dataset";

/**
 * Embedder determinista "bolsa de palabras hasheada" (sin modelo). Cada palabra
 * tiene un vector fijo; las palabras de un tema comparten un centroide, de modo
 * que el espacio vectorial queda agrupado por significado y es explorable en 3D.
 * Sólo existe para la demo: el motor acepta cualquier vector.
 */
export const DIM = 64;

function hash(str: string): number {
  let h = 2166136261;
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

function gauss(seed: number): Float32Array {
  const v = new Float32Array(DIM);
  let s = seed >>> 0;
  const next = () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  for (let i = 0; i < DIM; i++) v[i] = (next() + next() + next() - 1.5) * 2;
  return v;
}

const centroids = new Map<string, Float32Array>();
const wordTopic = new Map<string, string>();
for (const t of TOPICS) {
  centroids.set(t.id, gauss(hash("topic:" + t.id)));
  for (const w of t.words) wordTopic.set(w, t.id);
}

const wordCache = new Map<string, Float32Array>();
function wordVec(w: string): Float32Array {
  let v = wordCache.get(w);
  if (v) return v;
  v = gauss(hash("w:" + w));
  const topic = wordTopic.get(w);
  if (topic) {
    const c = centroids.get(topic)!;
    for (let i = 0; i < DIM; i++) v[i] = v[i]! * 0.45 + c[i]! * 1.4;
  }
  wordCache.set(w, v);
  return v;
}

export function tokenize(text: string): string[] {
  return text
    .toLowerCase()
    .normalize("NFC")
    .split(/[^\p{L}\p{N}]+/u)
    .filter((w) => w.length > 2);
}

export function embed(text: string): Float32Array {
  const out = new Float32Array(DIM);
  for (const w of tokenize(text)) {
    const v = wordVec(w);
    for (let i = 0; i < DIM; i++) out[i]! += v[i]!;
  }
  let n = 0;
  for (let i = 0; i < DIM; i++) n += out[i]! * out[i]!;
  n = Math.sqrt(n) || 1;
  for (let i = 0; i < DIM; i++) out[i]! /= n;
  return out;
}

export function topicOf(text: string): string | undefined {
  const votes = new Map<string, number>();
  for (const w of tokenize(text)) {
    const t = wordTopic.get(w);
    if (t) votes.set(t, (votes.get(t) ?? 0) + 1);
  }
  let best: string | undefined;
  let bn = 0;
  for (const [t, n] of votes) if (n > bn) ((best = t), (bn = n));
  return best;
}
