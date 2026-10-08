import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { useStore } from "../store";
import { agentPos, hex, now } from "../fx";

const PER = 8;
const WORKING_MS = 30_000; // mismo TTL que workingSet en el servidor

/** Working memory: los recuerdos recientes de cada agente orbitan junto a él hasta que caduca el TTL. */
export function WorkingRing() {
  const agents = useStore((s) => s.agents);
  const mesh = useRef<THREE.InstancedMesh>(null!);
  const dummy = useMemo(() => new THREE.Object3D(), []);
  const col = useMemo(() => new THREE.Color(), []);
  const total = Math.max(1, agents.length) * PER;

  useFrame(() => {
    const t = now();
    const { docs } = useStore.getState();
    const nowMs = Date.now();
    const recent = new Map<string, number[]>();
    for (let i = docs.length - 1; i >= 0; i--) {
      const d = docs[i]!;
      if (nowMs - d.t > WORKING_MS) continue;
      if (!d.alive) continue;
      const arr = recent.get(d.agent) ?? [];
      if (arr.length < PER) arr.push(i);
      recent.set(d.agent, arr);
    }
    let n = 0;
    agents.forEach((a, ai) => {
      const c = hex(a.color);
      const p = agentPos(ai, agents.length, t);
      (recent.get(a.id) ?? []).forEach((di, k) => {
        const d = docs[di]!;
        const life = 1 - (nowMs - d.t) / WORKING_MS;
        const ang = t * 1.3 + k * 0.8;
        dummy.position.set(p[0] + Math.cos(ang) * 2.6, p[1] + Math.sin(ang * 0.7) * 0.8, p[2] + Math.sin(ang) * 2.6);
        dummy.scale.setScalar(0.12 + 0.12 * life);
        dummy.updateMatrix();
        mesh.current.setMatrixAt(n, dummy.matrix);
        col.setRGB(c[0] * (0.4 + life), c[1] * (0.4 + life), c[2] * (0.4 + life));
        mesh.current.setColorAt(n, col);
        n++;
      });
    });
    for (let i = n; i < total; i++) {
      dummy.scale.setScalar(0);
      dummy.updateMatrix();
      mesh.current.setMatrixAt(i, dummy.matrix);
    }
    mesh.current.instanceMatrix.needsUpdate = true;
    if (mesh.current.instanceColor) mesh.current.instanceColor.needsUpdate = true;
  });

  return (
    <instancedMesh key={total} ref={mesh} args={[undefined, undefined, total]} frustumCulled={false}>
      <sphereGeometry args={[1, 8, 8]} />
      <meshBasicMaterial toneMapped={false} />
    </instancedMesh>
  );
}
