import * as THREE from "three";
import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#a59a90";
const CREAM = "#f3ece2";
const PINK = "#f2a2ad";

/** The tail's line: down and back to just off the ground, then a curl up. */
const TAIL = new THREE.TubeGeometry(
  new THREE.CatmullRomCurve3(
    [
      [0, 0, 0],
      [0.01, -0.09, -0.1],
      [0.05, -0.17, -0.22],
      [0.14, -0.19, -0.32],
      [0.25, -0.15, -0.33],
      [0.3, -0.06, -0.26],
      [0.27, 0.0, -0.2],
    ].map(([x, y, z]) => new THREE.Vector3(x, y, z)),
  ),
  14,
  0.018,
  4,
);

export const mouse: AnimalDesign = {
  coat: COAT,
  accent: "#f2a33a",
  top: 1.28,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.05, 0.95, 1.05]}>
        <sphereGeometry args={[0.24, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* Big round ears, pink inside, set wide. */}
      {[-1, 1].map((s) => (
        <group
          key={s}
          position={[s * 0.2, 0.18, -0.03]}
          rotation={[-0.15, 0, s * -0.45]}
        >
          <mesh scale={[1, 1, 0.3]}>
            <sphereGeometry args={[0.16, 10, 7]} />
            <Mat color={COAT} />
          </mesh>
          <mesh position-z={0.03} scale={[1, 1, 0.25]}>
            <sphereGeometry args={[0.11, 10, 7]} />
            <Mat color={PINK} />
          </mesh>
        </group>
      ))}
      {/* A pointed snout ending in a pink nose, with whiskers. */}
      <mesh position={[0, -0.07, 0.24]} rotation-x={Math.PI / 2}>
        <coneGeometry args={[0.1, 0.17, 8]} />
        <Mat color={CREAM} />
      </mesh>
      <mesh position={[0, -0.07, 0.33]}>
        <sphereGeometry args={[0.03, 6, 5]} />
        <Mat color="#e0707f" />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.1, -0.075, 0.27]}
          rotation={[0, s * 0.35, s * 0.1]}
        >
          <boxGeometry args={[0.16, 0.007, 0.007]} />
          <meshBasicMaterial color="#5a514b" />
        </mesh>
      ))}
      <DotEyes eyes={eyes} y={0.03} x={0.095} z={0.215} r={0.05} />
      <Blush y={-0.05} x={0.155} z={0.185} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1, 1.15, 0.95]}>
        <sphereGeometry args={[0.22, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, -0.03, 0.12]} scale={[0.75, 1, 0.5]}>
        <sphereGeometry args={[0.19, 10, 8]} />
        <Mat color={CREAM} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={PINK} />,
  Foot: () => (
    <mesh scale={[0.9, 0.7, 1.35]}>
      <sphereGeometry args={[0.08, 8, 6]} />
      <Mat color={PINK} />
    </mesh>
  ),
  /** A thin pink tail, trailing behind and curling up at its tip. */
  Tail: () => (
    <mesh geometry={TAIL}>
      <Mat color={PINK} />
    </mesh>
  ),
};
