import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#c98a4b";
const WHITE = "#fbf6ee";
const EAR = "#6e4429";
const SADDLE = "#3b2c25";

export const dog: AnimalDesign = {
  coat: COAT,
  accent: "#3d7fd6",
  top: 1.13,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.05, 0.95, 1]}>
        <sphereGeometry args={[0.25, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* A white blaze up the face, into a round muzzle with a big nose. */}
      <mesh position={[0, 0.06, 0.17]} scale={[0.3, 1, 0.5]}>
        <sphereGeometry args={[0.16, 8, 6]} />
        <Mat color={WHITE} />
      </mesh>
      <mesh position={[0, -0.08, 0.2]} scale={[1.15, 0.8, 0.9]}>
        <sphereGeometry args={[0.11, 10, 8]} />
        <Mat color={WHITE} />
      </mesh>
      <mesh position={[0, -0.04, 0.3]} scale={[1.3, 0.9, 1]}>
        <sphereGeometry args={[0.038, 7, 5]} />
        <Mat color="#1d1d1f" />
      </mesh>
      <mesh position={[0.02, -0.15, 0.24]} rotation-x={0.4}>
        <capsuleGeometry args={[0.025, 0.03, 3, 6]} />
        <Mat color="#ef7a8a" />
      </mesh>
      {/* Long floppy ears hanging down the sides. */}
      {[-1, 1].map((s) => (
        <group
          key={s}
          position={[s * 0.21, 0.15, 0.02]}
          rotation={[0.15, 0, s * 0.22]}
        >
          <mesh position-y={-0.14} scale={[0.45, 1, 0.75]}>
            <sphereGeometry args={[0.15, 8, 6]} />
            <Mat color={EAR} />
          </mesh>
        </group>
      ))}
      <DotEyes eyes={eyes} y={0.04} x={0.1} z={0.215} />
      <Blush y={-0.04} x={0.16} z={0.19} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* A dark saddle over the back, a white bib in front. */}
      <mesh position={[0, 0.06, -0.09]} scale={[0.9, 0.9, 0.7]}>
        <sphereGeometry args={[0.22, 10, 8]} />
        <Mat color={SADDLE} />
      </mesh>
      <mesh position={[0, 0.02, 0.13]} scale={[0.72, 1, 0.5]}>
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
    <group rotation-x={-0.35}>
      <mesh position-y={0.15}>
        <capsuleGeometry args={[0.042, 0.22, 3, 6]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position-y={0.31}>
        <sphereGeometry args={[0.05, 7, 5]} />
        <Mat color={WHITE} />
      </mesh>
    </group>
  ),
};
