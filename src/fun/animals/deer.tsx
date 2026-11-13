import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#a8693d";
const CREAM = "#f6ead6";
const HOOF = "#3b2a22";
const ANTLER = "#f2e3c4";

/** White spots down the back, on the torso's surface. */
const SPOTS: [number, number, number][] = [
  [-0.1, 0.47, -0.18],
  [0.09, 0.42, -0.2],
  [-0.04, 0.32, -0.215],
];

export const deer: AnimalDesign = {
  coat: COAT,
  accent: "#3fae8c",
  top: 1.38,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1, 0.95, 1.05]}>
        <sphereGeometry args={[0.24, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* A pale muzzle with a dark nose. */}
      <mesh position={[0, -0.09, 0.19]} scale={[1.1, 0.8, 1]}>
        <sphereGeometry args={[0.11, 8, 6]} />
        <Mat color={CREAM} />
      </mesh>
      <mesh position={[0, -0.06, 0.295]} scale={[1.3, 0.9, 0.8]}>
        <sphereGeometry args={[0.035, 6, 5]} />
        <Mat color={HOOF} />
      </mesh>
      {/* Leaf ears out to the sides, and branching antlers. */}
      {[-1, 1].map((s) => (
        <group key={s}>
          <mesh
            position={[s * 0.25, 0.08, -0.02]}
            rotation={[0, 0, s * -1.15]}
            scale={[0.55, 1.3, 0.3]}
          >
            <sphereGeometry args={[0.09, 8, 6]} />
            <Mat color={COAT} />
          </mesh>
          <group position={[s * 0.09, 0.19, -0.03]} rotation-z={s * -0.4}>
            <mesh position-y={0.15}>
              <cylinderGeometry args={[0.02, 0.03, 0.3, 5]} />
              <Mat color={ANTLER} />
            </mesh>
            <mesh position={[s * 0.042, 0.17, 0]} rotation-z={s * -0.7}>
              <cylinderGeometry args={[0.014, 0.02, 0.13, 5]} />
              <Mat color={ANTLER} />
            </mesh>
            <mesh position={[s * -0.028, 0.26, 0]} rotation-z={s * 0.6}>
              <cylinderGeometry args={[0.013, 0.018, 0.1, 5]} />
              <Mat color={ANTLER} />
            </mesh>
          </group>
        </group>
      ))}
      <DotEyes eyes={eyes} y={0.04} x={0.1} z={0.21} r={0.048} />
      <Blush y={-0.04} x={0.165} z={0.18} />
    </group>
  ),
  Torso: () => (
    <group>
      <group position-y={(RIG.hip + RIG.chest) / 2}>
        <mesh scale={[1, 1.15, 0.95]}>
          <sphereGeometry args={[0.23, 12, 10]} />
          <Mat color={COAT} />
        </mesh>
      </group>
      {SPOTS.map((p, i) => (
        <mesh key={i} position={p} scale={[1, 1, 0.45]}>
          <sphereGeometry args={[0.042, 6, 4]} />
          <Mat color="#fffaf0" />
        </mesh>
      ))}
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={HOOF} />,
  Foot: () => (
    <mesh scale={[0.95, 0.8, 1.2]}>
      <sphereGeometry args={[0.08, 8, 6]} />
      <Mat color={HOOF} />
    </mesh>
  ),
  /** A short tail flicked up, white beneath. */
  Tail: () => (
    <mesh position={[0, 0.05, -0.04]} rotation-x={-0.6} scale={[1, 1.5, 0.8]}>
      <sphereGeometry args={[0.055, 7, 5]} />
      <Mat color={CREAM} />
    </mesh>
  ),
};
