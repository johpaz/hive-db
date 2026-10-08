import { Canvas } from "@react-three/fiber";
import { OrbitControls, Stars } from "@react-three/drei";
import { Bloom, EffectComposer } from "@react-three/postprocessing";
import { PointCloud } from "./PointCloud";
import { Beams, Rings } from "./Effects";
import { Agents } from "./Agents";
import { EventHelix } from "./EventHelix";
import { WorkingRing } from "./WorkingRing";
import { CameraRig } from "./CameraRig";

export function Scene() {
  return (
    <Canvas
      camera={{ position: [34, 20, 50], fov: 52, near: 0.1, far: 400 }}
      dpr={[1, 2]}
      gl={{ antialias: false, powerPreference: "high-performance" }}
      raycaster={{ params: { Points: { threshold: 0.45 } } as never }}
    >
      <color attach="background" args={["#05060c"]} />
      <fog attach="fog" args={["#05060c", 70, 160]} />
      <Stars radius={110} depth={40} count={2500} factor={3} fade speed={0.4} />
      <PointCloud />
      <Beams />
      <Rings />
      <Agents />
      <EventHelix />
      <WorkingRing />
      <CameraRig />
      <OrbitControls makeDefault enableDamping dampingFactor={0.08} autoRotate autoRotateSpeed={0.35}  maxDistance={110} minDistance={8} />
      <EffectComposer>
        <Bloom intensity={0.7} luminanceThreshold={0.32} luminanceSmoothing={0.4} mipmapBlur />
      </EffectComposer>
    </Canvas>
  );
}
