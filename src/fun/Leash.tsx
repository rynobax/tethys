import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import type { Anchors } from "./Character";
import { LEASH_LENGTH, type Sim } from "./sim";

const SEGMENTS = 14;
const RADIUS = 0.018;
const UP = new THREE.Vector3(0, 1, 0);

/**
 * From a parent's hand to its child's collar, sagging by however much slack
 * there is. Short cylinders re-laid each frame rather than a geometry rebuilt,
 * so it allocates nothing while it swings.
 */
export function Leash({
  parentId,
  childId,
  sim,
  anchors,
  color,
}: {
  parentId: string;
  childId: string;
  sim: Sim;
  anchors: Anchors;
  color: string;
}) {
  const group = useRef<THREE.Group>(null);
  const segments = useRef<(THREE.Mesh | null)[]>([]);
  const scratch = useMemo(
    () => ({
      from: new THREE.Vector3(),
      to: new THREE.Vector3(),
      control: new THREE.Vector3(),
      points: Array.from({ length: SEGMENTS + 1 }, () => new THREE.Vector3()),
      dir: new THREE.Vector3(),
    }),
    [],
  );

  useFrame(() => {
    const g = group.current;
    if (!g) return;
    const hand = anchors.get(parentId)?.hand;
    const collar = anchors.get(childId)?.collar;
    g.visible = !!hand && !!collar && sim.leashed(childId, parentId);
    if (!g.visible || !hand || !collar) return;

    const { from, to, control, points, dir } = scratch;
    hand.getWorldPosition(from);
    collar.getWorldPosition(to);
    // A quadratic curve's midpoint drops by half its control point's drop.
    const slack = Math.max(0, LEASH_LENGTH - from.distanceTo(to));
    const midY = (from.y + to.y) / 2;
    const sag = Math.min(slack * 0.35 + 0.03, midY - 0.03);
    control.addVectors(from, to).multiplyScalar(0.5);
    control.y -= sag * 2;
    for (let i = 0; i <= SEGMENTS; i++) {
      const t = i / SEGMENTS;
      const a = (1 - t) * (1 - t);
      const b = 2 * (1 - t) * t;
      const c = t * t;
      points[i].set(
        a * from.x + b * control.x + c * to.x,
        a * from.y + b * control.y + c * to.y,
        a * from.z + b * control.z + c * to.z,
      );
    }
    for (let i = 0; i < SEGMENTS; i++) {
      const m = segments.current[i];
      if (!m) continue;
      dir.subVectors(points[i + 1], points[i]);
      const len = dir.length();
      m.position.addVectors(points[i], points[i + 1]).multiplyScalar(0.5);
      m.quaternion.setFromUnitVectors(UP, dir.normalize());
      m.scale.set(1, len + 0.004, 1);
    }
  });

  return (
    <group ref={group} visible={false}>
      {Array.from({ length: SEGMENTS }, (_, i) => (
        <mesh
          key={i}
          ref={(m) => {
            segments.current[i] = m;
          }}
        >
          <cylinderGeometry args={[RADIUS, RADIUS, 1, 5]} />
          <meshStandardMaterial color={color} flatShading roughness={0.8} />
        </mesh>
      ))}
    </group>
  );
}
