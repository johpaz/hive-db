import { useMemo, useRef } from "react";
import { useFrame, type ThreeEvent } from "@react-three/fiber";
import { Html } from "@react-three/drei";
import * as THREE from "three";
import { useStore } from "../store";
import { hex, now } from "../fx";

const N = 90;
const KIND_COLOR: Record<string, string> = {
  Fact: "#ffb020",
  ToolCall: "#4de2ff",
  MemoryInvalidate: "#ff3d4a",
};

function helixPos(i: number, t: number, out: THREE.Vector3) {
  const a = i * 0.62 + t * 0.35;
  return out.set(-32 + Math.cos(a) * 2.4, 15 - i * 0.34, Math.sin(a) * 2.4);
}

/** Cadena causal de un evento (hacia atrás por `causation`). */
export function causalChain(events: { seq: number; causation?: number }[], seq: number | null): Set<number> {
  const chain = new Set<number>();
  if (seq == null) return chain;
  const bySeq = new Map(events.map((e) => [e.seq, e]));
  let cur: number | undefined = seq;
  while (cur !== undefined && !chain.has(cur) && bySeq.has(cur)) {
    chain.add(cur);
    cur = bySeq.get(cur)!.causation;
  }
  return chain;
}

/** El event-log como hélice de tiempo; las líneas son enlaces `causation` reales. */
export function EventHelix() {
  const events = useStore((s) => s.events);
  const selectedEvent = useStore((s) => s.selectedEvent);
  const mesh = useRef<THREE.InstancedMesh>(null!);
  const dummy = useMemo(() => new THREE.Object3D(), []);
  const col = useMemo(() => new THREE.Color(), []);
  const v1 = useMemo(() => new THREE.Vector3(), []);
  const v2 = useMemo(() => new THREE.Vector3(), []);
  const chain = useMemo(() => causalChain(events, selectedEvent), [events, selectedEvent]);

  const lineGeo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(new Float32Array(N * 6), 3));
    g.setAttribute("color", new THREE.BufferAttribute(new Float32Array(N * 6), 3));
    g.setDrawRange(0, 0);
    return g;
  }, []);
  const lineMat = useMemo(
    () => new THREE.LineBasicMaterial({ vertexColors: true, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false }),
    []
  );

  useFrame(() => {
    const t = now();
    const idxBySeq = new Map<number, number>();
    events.forEach((e, i) => i < N && idxBySeq.set(e.seq, i));
    for (let i = 0; i < N; i++) {
      const e = events[i];
      if (!e) {
        dummy.scale.setScalar(0);
        dummy.updateMatrix();
        mesh.current.setMatrixAt(i, dummy.matrix);
        continue;
      }
      helixPos(i, t, dummy.position);
      const inChain = chain.has(e.seq);
      const s = (i === 0 ? 1.6 : 1) * (e.kind === "MemoryInvalidate" ? 1.3 : 1) * (inChain ? 1.8 : 1);
      dummy.scale.setScalar(0.22 * s);
      dummy.updateMatrix();
      mesh.current.setMatrixAt(i, dummy.matrix);
      const c = hex(KIND_COLOR[e.kind] ?? "#ffffff");
      const f = inChain ? 1.6 : selectedEvent != null ? 0.18 : 0.35 + (1 - i / N);
      col.setRGB(c[0] * f, c[1] * f, c[2] * f);
      mesh.current.setColorAt(i, col);
    }
    mesh.current.instanceMatrix.needsUpdate = true;
    if (mesh.current.instanceColor) mesh.current.instanceColor.needsUpdate = true;

    const pos = lineGeo.getAttribute("position") as THREE.BufferAttribute;
    const lc = lineGeo.getAttribute("color") as THREE.BufferAttribute;
    let n = 0;
    for (let i = 0; i < Math.min(events.length, N); i++) {
      const e = events[i]!;
      if (e.causation == null) continue;
      const j = idxBySeq.get(e.causation);
      if (j === undefined) continue;
      helixPos(i, t, v1);
      helixPos(j, t, v2);
      pos.setXYZ(n * 2, v1.x, v1.y, v1.z);
      pos.setXYZ(n * 2 + 1, v2.x, v2.y, v2.z);
      const hot = chain.has(e.seq) && chain.has(e.causation);
      const k = hot ? 1.2 : selectedEvent != null ? 0.05 : 0.22;
      const c = hot ? [1, 0.85, 0.3] : [0.7, 0.8, 1];
      lc.setXYZ(n * 2, c[0]! * k, c[1]! * k, c[2]! * k);
      lc.setXYZ(n * 2 + 1, c[0]! * k, c[1]! * k, c[2]! * k);
      n++;
    }
    pos.needsUpdate = lc.needsUpdate = true;
    lineGeo.setDrawRange(0, n * 2);
  });

  const onClick = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation();
    const ev = e.instanceId != null ? events[e.instanceId] : undefined;
    if (ev) useStore.getState().selectEvent(ev.seq);
  };

  return (
    <group>
      <lineSegments geometry={lineGeo} material={lineMat} frustumCulled={false} />
      <instancedMesh ref={mesh} args={[undefined, undefined, N]} frustumCulled={false} onClick={onClick}>
        <sphereGeometry args={[1, 10, 10]} />
        <meshBasicMaterial toneMapped={false} />
      </instancedMesh>
      <Html position={[-32, -16.5, 0]} center style={{ pointerEvents: "none" }}>
        <div className="agent-tag" style={{ borderColor: "#ffb020", color: "#ffb020" }}>
          event-log · append-only
        </div>
      </Html>
    </group>
  );
}
