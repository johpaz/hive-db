import { useMemo, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { beams, rings, now } from "../fx";

const MAX_BEAMS = 400;
const MAX_RINGS = 48;

export function Beams() {
  const geo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(new Float32Array(MAX_BEAMS * 6), 3));
    g.setAttribute("color", new THREE.BufferAttribute(new Float32Array(MAX_BEAMS * 6), 3));
    g.setDrawRange(0, 0);
    return g;
  }, []);
  const mat = useMemo(
    () =>
      new THREE.LineBasicMaterial({
        vertexColors: true,
        transparent: true,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
      }),
    []
  );

  useFrame(() => {
    const t = now();
    const pos = geo.getAttribute("position") as THREE.BufferAttribute;
    const col = geo.getAttribute("color") as THREE.BufferAttribute;
    let n = 0;
    for (let i = beams.length - 1; i >= 0; i--) {
      if (t - beams[i]!.t0 > beams[i]!.dur * 1.8 + 0.2) beams.splice(i, 1);
    }
    for (const b of beams) {
      const p = (t - b.t0) / b.dur;
      if (p < 0 || n >= MAX_BEAMS) continue;
      const head = Math.min(1, 1 - Math.pow(1 - Math.min(p, 1), 3));
      const tail = Math.max(0, Math.min(1, (p - 0.55) * 1.0));
      const fade = p > 1 ? Math.max(0, 1 - (p - 1) / 0.8) : 1;
      const boost = (b.width ?? 1) * fade;
      const h = head;
      const tl = Math.min(tail, h);
      pos.setXYZ(n * 2, b.from[0] + (b.to[0] - b.from[0]) * tl, b.from[1] + (b.to[1] - b.from[1]) * tl, b.from[2] + (b.to[2] - b.from[2]) * tl);
      pos.setXYZ(n * 2 + 1, b.from[0] + (b.to[0] - b.from[0]) * h, b.from[1] + (b.to[1] - b.from[1]) * h, b.from[2] + (b.to[2] - b.from[2]) * h);
      col.setXYZ(n * 2, b.color[0] * boost * 0.15, b.color[1] * boost * 0.15, b.color[2] * boost * 0.15);
      col.setXYZ(n * 2 + 1, b.color[0] * boost, b.color[1] * boost, b.color[2] * boost);
      n++;
    }
    pos.needsUpdate = col.needsUpdate = true;
    geo.setDrawRange(0, n * 2);
  });

  return <lineSegments geometry={geo} material={mat} frustumCulled={false} />;
}

export function Rings() {
  const { camera } = useThree();
  const refs = useRef<(THREE.Mesh | null)[]>([]);
  const geo = useMemo(() => new THREE.RingGeometry(0.92, 1, 64), []);
  const mats = useMemo(
    () =>
      Array.from({ length: MAX_RINGS }, () =>
        new THREE.MeshBasicMaterial({ transparent: true, blending: THREE.AdditiveBlending, depthWrite: false, side: THREE.DoubleSide })
      ),
    []
  );

  useFrame(() => {
    const t = now();
    for (let i = rings.length - 1; i >= 0; i--) if (t - rings[i]!.t0 > rings[i]!.dur) rings.splice(i, 1);
    let n = 0;
    for (const r of rings) {
      const p = (t - r.t0) / r.dur;
      if (p < 0 || n >= MAX_RINGS) continue;
      const m = refs.current[n]!;
      m.visible = true;
      m.position.set(r.at[0], r.at[1], r.at[2]);
      m.quaternion.copy(camera.quaternion);
      const s = r.maxR * (1 - Math.pow(1 - p, 3));
      m.scale.setScalar(Math.max(0.01, s));
      const a = (1 - p) * 0.55;
      (m.material as THREE.MeshBasicMaterial).color.setRGB(r.color[0] * a, r.color[1] * a, r.color[2] * a);
      n++;
    }
    for (let i = n; i < MAX_RINGS; i++) refs.current[i]!.visible = false;
  });

  return (
    <>
      {mats.map((m, i) => (
        <mesh key={i} ref={(el) => (refs.current[i] = el)} geometry={geo} material={m} visible={false} frustumCulled={false} />
      ))}
    </>
  );
}
