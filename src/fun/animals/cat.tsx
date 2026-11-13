import * as THREE from "three";
import { mergeGeometries } from "three/examples/jsm/utils/BufferGeometryUtils.js";
import {
  Blush,
  DotEyes,
  Mat,
  RIG,
  SimpleArm,
  stripes,
  type AnimalDesign,
  type Stripe,
} from "./rig";

const COAT = "#9aa1aa";
const TABBY = "#5f6670";
const WHITE = "#f7f5f1";
const PINK = "#f19aa6";

/** The tabby's barred brow, and bars down each side of its back. */
const BROW = stripes(0.25, [
  { up: true, yaw: 0, from: 0.6, to: 1.35 },
  ...[-1, 1].map((s): Stripe => ({
    up: true,
    yaw: s * 0.24,
    from: 0.6,
    to: 1.05,
  })),
]);
const BACK = stripes(0.23, [
  ...[-1, 1].flatMap((s): Stripe[] => [
    { up: true, yaw: s * 2.2, from: -0.2, to: 0.8 },
    { up: true, yaw: s * 1.7, from: -0.1, to: 0.6 },
  ]),
  { up: true, yaw: Math.PI, from: -0.3, to: 1.0, w: 0.026 },
]);

/** Two whiskers fanning out from each cheek. */
const WHISKERS = mergeGeometries(
  [-1, 1].flatMap((s) =>
    [0.12, -0.12].map((tilt) =>
      new THREE.BoxGeometry(0.15, 0.007, 0.007)
        .rotateZ(s * tilt)
        .rotateY(s * -0.4)
        .translate(s * 0.16, -0.07 + tilt * 0.12, 0.2),
    ),
  ),
)!;

export const cat: AnimalDesign = {
  coat: COAT,
  accent: "#f2a93b",
  top: 1.22,
  Head: ({ eyes }) => (
    <group>
      <group scale={[1.1, 0.92, 1]}>
        <mesh>
          <sphereGeometry args={[0.25, 12, 10]} />
          <Mat color={COAT} />
        </mesh>
        <mesh geometry={BROW}>
          <Mat color={TABBY} />
        </mesh>
      </group>
      {/* A small white muzzle in two puffs, a pink nose and whiskers. */}
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.05, -0.08, 0.2]} scale={[1, 0.8, 0.8]}>
          <sphereGeometry args={[0.07, 8, 6]} />
          <Mat color={WHITE} />
        </mesh>
      ))}
      <mesh position={[0, -0.04, 0.255]} rotation-x={Math.PI}>
        <coneGeometry args={[0.03, 0.035, 3]} />
        <Mat color={PINK} />
      </mesh>
      <mesh geometry={WHISKERS}>
        <meshBasicMaterial color="#3b3f45" />
      </mesh>
      {/* Upright triangular ears, pink inside. */}
      {[-1, 1].map((s) => (
        <group key={s} position={[s * 0.14, 0.17, 0]} rotation-z={s * -0.3}>
          <mesh position-y={0.07} scale={[1, 1, 0.55]}>
            <coneGeometry args={[0.1, 0.18, 4]} />
            <Mat color={COAT} />
          </mesh>
          <mesh position={[0, 0.06, 0.03]} scale={[1, 1, 0.3]}>
            <coneGeometry args={[0.06, 0.12, 4]} />
            <Mat color={PINK} />
          </mesh>
        </group>
      ))}
      <DotEyes eyes={eyes} y={0.03} x={0.1} z={0.215} color="#3a5a2e" />
      <Blush y={-0.05} x={0.17} z={0.19} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <group scale={[1, 1.15, 0.95]}>
        <mesh>
          <sphereGeometry args={[0.23, 12, 10]} />
          <Mat color={COAT} />
        </mesh>
        <mesh geometry={BACK}>
          <Mat color={TABBY} />
        </mesh>
      </group>
      <mesh position={[0, 0.02, 0.13]} scale={[0.7, 1, 0.5]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color={WHITE} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={WHITE} />,
  Foot: () => (
    <mesh scale={[1, 0.75, 1.25]}>
      <sphereGeometry args={[0.085, 8, 6]} />
      <Mat color={WHITE} />
    </mesh>
  ),
  Tail: () => (
    <group rotation={[-0.6, 0, 0.3]}>
      <mesh position-y={0.24}>
        <capsuleGeometry args={[0.035, 0.42, 3, 6]} />
        <Mat color={COAT} />
      </mesh>
      {/* The tip curls over forward, dark like the bars. */}
      <mesh position={[0, 0.49, 0.04]} rotation-x={1}>
        <capsuleGeometry args={[0.037, 0.08, 3, 6]} />
        <Mat color={TABBY} />
      </mesh>
    </group>
  ),
};
