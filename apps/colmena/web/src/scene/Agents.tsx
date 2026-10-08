import { useRef } from "react";
import { useFrame } from "@react-three/fiber";
import { Html } from "@react-three/drei";
import * as THREE from "three";
import { agentFlash, agentPos, now } from "../fx";
import { useStore } from "../store";

export function Agents() {
  const agents = useStore((s) => s.agents);
  const refs = useRef<(THREE.Group | null)[]>([]);
  const cores = useRef<(THREE.Mesh | null)[]>([]);

  useFrame(() => {
    const t = now();
    agents.forEach((a, i) => {
      const g = refs.current[i];
      const core = cores.current[i];
      if (!g || !core) return;
      const p = agentPos(i, agents.length, t);
      g.position.set(p[0], p[1], p[2]);
      const f = agentFlash.get(a.id);
      const k = f ? Math.max(0, 1 - (t - f.t0) / 0.9) : 0;
      core.scale.setScalar(1 + k * 0.9);
      const m = core.material as THREE.MeshBasicMaterial;
      if (f && k > 0) m.color.setRGB(f.color[0] * (0.7 + k), f.color[1] * (0.7 + k), f.color[2] * (0.7 + k));
      else m.color.set(a.color);
      g.rotation.y = t * 0.8;
    });
  });

  return (
    <>
      {agents.map((a, i) => (
        <group key={a.id} ref={(el) => (refs.current[i] = el)}>
          <mesh ref={(el) => (cores.current[i] = el)}>
            <icosahedronGeometry args={[0.7, 1]} />
            <meshBasicMaterial color={a.color} />
          </mesh>
          <mesh>
            <icosahedronGeometry args={[1.25, 1]} />
            <meshBasicMaterial color={a.color} wireframe transparent opacity={0.35} />
          </mesh>
          <Html center position={[0, 2.2, 0]} style={{ pointerEvents: "none" }}>
            <div className="agent-tag" style={{ borderColor: a.color, color: a.color }}>
              {a.label}
            </div>
          </Html>
        </group>
      ))}
    </>
  );
}
