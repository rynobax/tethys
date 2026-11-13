import * as THREE from "three";
import { mergeGeometries } from "three/examples/jsm/utils/BufferGeometryUtils.js";
import {
  DotEyes,
  Mat,
  RIG,
  stripes,
  type AnimalDesign,
  type Stripe,
} from "./rig";

const COAT = "#f29324";
const WHITE = "#fff6ea";
const STRIPE = "#2a1f1b";

/** Bars down the brow and over the crown, and swept back on the cheeks. */
const HEAD = stripes(0.25, [
  { up: true, yaw: 0, from: 0.55, to: 1.45, w: 0.028 },
  ...[-1, 1].flatMap((s): Stripe[] => [
    { up: true, yaw: s * 0.32, from: 0.5, to: 1.1, w: 0.024 },
    { up: true, yaw: s * 2.5, from: 0.25, to: 1.1 },
    { y: 0.0, yaw: s * 1.35, span: 0.7, w: 0.024 },
    { y: -0.08, yaw: s * 1.5, span: 0.5 },
  ]),
  { up: true, yaw: Math.PI, from: 0.15, to: 1.0, w: 0.026 },
]);

/** Bands wrapping round the back and sides, stopping short of the belly. */
const BODY = stripes(0.23, [
  { y: 0.14, yaw: Math.PI, span: 3.2, w: 0.026 },
  { y: 0.05, yaw: Math.PI + 0.3, span: 3.4, w: 0.026 },
  { y: -0.04, yaw: Math.PI - 0.3, span: 3.4, w: 0.026 },
  { y: -0.13, yaw: Math.PI, span: 3.0, w: 0.026 },
]);

/** Rings round a limb of radius `r`, at each of `ys`, in one mesh. */
function rings(r: number, ys: number[]): THREE.BufferGeometry {
  const parts = ys.map((y) =>
    new THREE.TorusGeometry(r, 0.016, 3, 8)
      .rotateX(Math.PI / 2)
      .translate(0, y, 0),
  );
  const merged = mergeGeometries(parts)!;
  for (const p of parts) p.dispose();
  return merged;
}

const ARM = rings(0.06, [-0.12, -0.2]);
const TAIL = rings(0.045, [0.16, 0.28]);

export const tiger: AnimalDesign = {
  coat: COAT,
  accent: "#2f9e72",
  top: 1.16,
  Head: ({ eyes }) => (
    <group>
      <group scale={[1.12, 0.95, 1]}>
        <mesh>
          <sphereGeometry args={[0.25, 12, 10]} />
          <Mat color={COAT} />
        </mesh>
        <mesh geometry={HEAD}>
          <Mat color={STRIPE} />
        </mesh>
      </group>
      {/* Puffed white cheeks and a pink nose. */}
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.1, -0.09, 0.16]} scale={[1, 0.75, 0.8]}>
          <sphereGeometry args={[0.11, 8, 6]} />
          <Mat color={WHITE} />
        </mesh>
      ))}
      <mesh position={[0, -0.04, 0.255]} scale={[1.3, 0.8, 0.8]}>
        <sphereGeometry args={[0.032, 6, 5]} />
        <Mat color="#c4545e" />
      </mesh>
      {/* Small round ears, pale inside. */}
      {[-1, 1].map((s) => (
        <group key={s} position={[s * 0.17, 0.18, 0]} rotation-z={s * -0.35}>
          <mesh scale={[1, 1, 0.55]}>
            <sphereGeometry args={[0.08, 8, 6]} />
            <Mat color={COAT} />
          </mesh>
          <mesh position-z={0.03} scale={[1, 1, 0.4]}>
            <sphereGeometry args={[0.048, 6, 5]} />
            <Mat color={WHITE} />
          </mesh>
        </group>
      ))}
      <DotEyes eyes={eyes} y={0.03} x={0.1} z={0.215} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <group scale={[1, 1.15, 0.95]}>
        <mesh>
          <sphereGeometry args={[0.23, 12, 10]} />
          <Mat color={COAT} />
        </mesh>
        <mesh geometry={BODY}>
          <Mat color={STRIPE} />
        </mesh>
      </group>
      <mesh position={[0, -0.02, 0.13]} scale={[0.75, 1, 0.5]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color={WHITE} />
      </mesh>
    </group>
  ),
  Arm: () => (
    <>
      <mesh position-y={-0.14}>
        <capsuleGeometry args={[0.06, 0.16, 3, 6]} />
        <Mat color={COAT} />
      </mesh>
      <mesh geometry={ARM}>
        <Mat color={STRIPE} />
      </mesh>
      <mesh position-y={-RIG.hand + 0.03}>
        <sphereGeometry args={[0.068, 7, 5]} />
        <Mat color={WHITE} />
      </mesh>
    </>
  ),
  Foot: () => (
    <mesh scale={[1, 0.75, 1.25]}>
      <sphereGeometry args={[0.085, 8, 6]} />
      <Mat color={WHITE} />
    </mesh>
  ),
  Tail: () => (
    <group rotation-x={-0.85}>
      <mesh position-y={0.2}>
        <capsuleGeometry args={[0.045, 0.34, 3, 6]} />
        <Mat color={COAT} />
      </mesh>
      <mesh geometry={TAIL}>
        <Mat color={STRIPE} />
      </mesh>
      <mesh position-y={0.39}>
        <sphereGeometry args={[0.052, 7, 5]} />
        <Mat color={STRIPE} />
      </mesh>
    </group>
  ),
};
