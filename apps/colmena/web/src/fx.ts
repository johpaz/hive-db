import type { XYZ } from "./types";

/** Efectos transitorios por frame (fuera de React para no re-renderizar). */
export const now = () => performance.now() / 1000;

export interface Beam {
  from: XYZ;
  to: XYZ;
  color: [number, number, number];
  t0: number;
  dur: number;
  width?: number;
}

export interface Ring {
  at: XYZ;
  color: [number, number, number];
  t0: number;
  dur: number;
  maxR: number;
}

export const beams: Beam[] = [];
export const rings: Ring[] = [];
export const agentFlash = new Map<string, { t0: number; color: [number, number, number] }>();

export function addBeam(b: Beam) {
  beams.push(b);
  if (beams.length > 400) beams.splice(0, beams.length - 400);
}
export function addRing(r: Ring) {
  rings.push(r);
  if (rings.length > 60) rings.splice(0, rings.length - 60);
}

export function hex(c: string): [number, number, number] {
  const n = parseInt(c.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

export const COLORS = {
  text: hex("#4de2ff"),
  vector: hex("#ffb020"),
  fused: hex("#ffffff"),
  dead: hex("#ff3d4a"),
  gateOk: hex("#7bf0a5"),
  gateNo: hex("#ff3d4a"),
};

/** Posición fija de cada agente: anillo alrededor de la nube. */
export function agentPos(index: number, total: number, t = 0): XYZ {
  const a = (index / Math.max(1, total)) * Math.PI * 2 + t * 0.12;
  return [Math.cos(a) * 19, 4 + Math.sin(t * 0.6 + index) * 1.2, Math.sin(a) * 19];
}
