import { Blush, DotEyes, Mat, RIG, type AnimalDesign } from "./rig";

const BACK = "#2c3440";
const WHITE = "#f6f4ee";
const ORANGE = "#f39a2b";

export const penguin: AnimalDesign = {
  coat: BACK,
  accent: "#e8485e",
  top: 1.14,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.05, 0.98, 1]}>
        <sphereGeometry args={[0.25, 12, 10]} />
        <Mat color={BACK} />
      </mesh>
      {/* A white face in two lobes, the dark cap coming to a point between. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.075, -0.03, 0.1]}
          scale={[1, 1.05, 0.95]}
        >
          <sphereGeometry args={[0.165, 10, 8]} />
          <Mat color={WHITE} />
        </mesh>
      ))}
      <mesh position={[0, -0.05, 0.28]} rotation-x={Math.PI / 2}>
        <coneGeometry args={[0.05, 0.11, 6]} />
        <Mat color={ORANGE} />
      </mesh>
      <DotEyes eyes={eyes} y={0.02} x={0.09} z={0.245} r={0.04} />
      <Blush y={-0.05} x={0.15} z={0.22} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.12, 1.25, 1]}>
        <sphereGeometry args={[0.24, 12, 10]} />
        <Mat color={BACK} />
      </mesh>
      <mesh position={[0, -0.02, 0.14]} scale={[0.86, 1.1, 0.55]}>
        <sphereGeometry args={[0.22, 10, 8]} />
        <Mat color={WHITE} />
      </mesh>
    </group>
  ),
  Arm: ({ side }) => (
    <group rotation-z={side * 0.12}>
      <mesh position-y={-0.15} scale={[0.32, 1.75, 0.75]}>
        <sphereGeometry args={[0.1, 8, 6]} />
        <Mat color={BACK} />
      </mesh>
    </group>
  ),
  Foot: () => (
    <mesh position={[0, -0.03, -0.03]}>
      <cylinderGeometry args={[0.125, 0.125, 0.03, 3, 1, false, -0.7, 1.4]} />
      <Mat color={ORANGE} />
    </mesh>
  ),
  Tail: () => (
    <mesh
      position={[0, -0.06, -0.05]}
      rotation-x={-Math.PI / 2 - 0.5}
      scale={[1, 1, 0.5]}
    >
      <coneGeometry args={[0.07, 0.14, 4]} />
      <Mat color={BACK} />
    </mesh>
  ),
};
