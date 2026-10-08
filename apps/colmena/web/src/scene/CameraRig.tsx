import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useStore } from "../store";
import { STEPS } from "../tour";

/** Durante el tour mueve cámara y objetivo suavemente; al terminar devuelve el control. */
export function CameraRig() {
  const tour = useStore((s) => s.tour);
  const { camera, controls } = useThree();
  const goal = useRef({ pos: new THREE.Vector3(), target: new THREE.Vector3() });

  useEffect(() => {
    const c = controls as unknown as { autoRotate: boolean } | null;
    if (c) c.autoRotate = tour === null;
    if (tour !== null) {
      const s = STEPS[tour]!;
      goal.current.pos.set(...s.cam);
      goal.current.target.set(...s.target);
    }
  }, [tour, controls]);

  useFrame((_, dt) => {
    if (tour === null || !controls) return;
    const c = controls as unknown as { target: THREE.Vector3; update(): void };
    const k = 1 - Math.pow(0.02, dt);
    camera.position.lerp(goal.current.pos, k);
    c.target.lerp(goal.current.target, k);
    c.update();
  });

  return null;
}
