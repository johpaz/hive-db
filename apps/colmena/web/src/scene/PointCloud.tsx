import { useEffect, useMemo, useRef } from "react";
import { useFrame, type ThreeEvent } from "@react-three/fiber";
import * as THREE from "three";
import { useStore } from "../store";
import { hex, now } from "../fx";

const CAP = 12000;

const vert = /* glsl */ `
  attribute float aBirth;
  attribute float aDeath;
  attribute float aGlow;
  uniform float uTime;
  uniform float uScale;
  varying vec3 vColor;
  varying float vAlpha;
  varying float vGlow;
  void main() {
    float age = uTime - aBirth;
    float size = 1.9;
    vec3 col = color;
    float alpha = 1.0;
    if (age < 0.0) { size = 0.0; }
    size += exp(-max(age, 0.0) * 1.8) * 5.0;
    if (aDeath > 0.0) {
      float d = uTime - aDeath;
      float k = clamp(d / 1.6, 0.0, 1.0);
      col = mix(vec3(1.0, 0.18, 0.25), col * 0.2, k);
      size *= (1.0 - k) * (1.0 + (1.0 - k) * 1.5);
      alpha = 1.0 - k;
    }
    float g = 0.0;
    if (aGlow > 0.0 && uTime > aGlow) { g = exp(-(uTime - aGlow) * 0.35); if (uTime - aGlow > 9.0) g = 0.0; }
    col = mix(col, vec3(1.0), g * 0.85);
    size *= 1.0 + g * 1.6;
    vColor = col; vAlpha = alpha; vGlow = g;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_PointSize = max(0.0, size * uScale / -mv.z);
    gl_Position = projectionMatrix * mv;
  }
`;
const frag = /* glsl */ `
  varying vec3 vColor;
  varying float vAlpha;
  varying float vGlow;
  void main() {
    float d = length(gl_PointCoord - 0.5);
    if (d > 0.5) discard;
    float core = smoothstep(0.5, 0.0, d);
    float a = (core * core * 0.8 + 0.25 * core) * vAlpha;
    gl_FragColor = vec4(vColor * (1.0 + vGlow), a);
  }
`;

export function PointCloud() {
  const ref = useRef<THREE.Points>(null!);
  const seen = useRef(new Map<string, { born: number; died: number }>());
  const first = useRef(true);

  const geo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(new Float32Array(CAP * 3), 3));
    g.setAttribute("color", new THREE.BufferAttribute(new Float32Array(CAP * 3), 3));
    g.setAttribute("aBirth", new THREE.BufferAttribute(new Float32Array(CAP), 1));
    g.setAttribute("aDeath", new THREE.BufferAttribute(new Float32Array(CAP), 1));
    g.setAttribute("aGlow", new THREE.BufferAttribute(new Float32Array(CAP), 1));
    g.setDrawRange(0, 0);
    return g;
  }, []);

  const mat = useMemo(
    () =>
      new THREE.ShaderMaterial({
        vertexShader: vert,
        fragmentShader: frag,
        vertexColors: true,
        transparent: true,
        depthWrite: false,
        blending: THREE.AdditiveBlending,
        uniforms: { uTime: { value: 0 }, uScale: { value: 420 } },
      }),
    []
  );

  const docs = useStore((s) => s.docs);
  const version = useStore((s) => s.docsVersion);
  const topics = useStore((s) => s.topics);
  const highlight = useStore((s) => s.highlight);

  useEffect(() => {
    if (!docs.length) return;
    const colors = new Map(topics.map((t) => [t.id, hex(t.color)]));
    const pos = geo.getAttribute("position") as THREE.BufferAttribute;
    const col = geo.getAttribute("color") as THREE.BufferAttribute;
    const birth = geo.getAttribute("aBirth") as THREE.BufferAttribute;
    const death = geo.getAttribute("aDeath") as THREE.BufferAttribute;
    const t = now();
    const n = Math.min(docs.length, CAP);
    for (let i = 0; i < n; i++) {
      const d = docs[i]!;
      let rec = seen.current.get(d.id);
      if (!rec) {
        rec = { born: first.current ? -1000 : t, died: 0 };
        seen.current.set(d.id, rec);
      }
      if (!d.alive && !rec.died) rec.died = first.current ? -1000 : t;
      const c = colors.get(d.topic) ?? [1, 1, 1];
      pos.setXYZ(i, d.xyz[0], d.xyz[1], d.xyz[2]);
      col.setXYZ(i, c[0], c[1], c[2]);
      birth.setX(i, rec.born);
      death.setX(i, rec.died);
    }
    pos.needsUpdate = col.needsUpdate = birth.needsUpdate = death.needsUpdate = true;
    geo.setDrawRange(0, n);
    first.current = false;
  }, [docs, version, topics, geo]);

  useEffect(() => {
    const idx = useStore.getState().docIndex;
    const glow = geo.getAttribute("aGlow") as THREE.BufferAttribute;
    for (const [id, t] of highlight) {
      const i = idx.get(id);
      if (i !== undefined && i < CAP) glow.setX(i, t);
    }
    glow.needsUpdate = true;
  }, [highlight, geo]);

  useFrame(({ size }) => {
    mat.uniforms.uTime!.value = now();
    mat.uniforms.uScale!.value = size.height * 0.62;
  });

  const onClick = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation();
    const i = e.index;
    if (i === undefined) return;
    const d = useStore.getState().docs[i];
    if (d) useStore.getState().select(d.id);
  };

  return (
    <points
      ref={ref}
      geometry={geo}
      material={mat}
      frustumCulled={false}
      onClick={onClick}
      onPointerMissed={() => useStore.getState().select(null)}
    />
  );
}
