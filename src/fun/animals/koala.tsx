import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#9ea4ab";
const FLUFF = "#f3f1ec";
const NOSE = "#35303a";
const PAW = "#6f747c";

/** A koala: grey, with big fluffy ears out to the sides and a large dark nose. */
export const koala: AnimalDesign = {
  coat: COAT,
  accent: "#ef7a6a",
  top: 1.15,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.15, 0.95, 1]}>
        <sphereGeometry args={[0.26, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* Big round ears, spread wide, with a white fringe facing forward. */}
      {[-1, 1].map((s) => (
        <group
          key={s}
          position={[s * 0.27, 0.13, -0.02]}
          rotation-z={s * -0.35}
        >
          <mesh scale={[1, 1, 0.55]}>
            <sphereGeometry args={[0.135, 10, 8]} />
            <Mat color={COAT} />
          </mesh>
          <mesh position={[s * 0.01, 0, 0.045]} scale={[1, 1, 0.4]}>
            <sphereGeometry args={[0.1, 9, 7]} />
            <Mat color={FLUFF} />
          </mesh>
        </group>
      ))}
      {/* The large oval nose, from between the eyes to the mouth. */}
      <mesh position={[0, -0.04, 0.245]} scale={[0.85, 1.2, 0.65]}>
        <sphereGeometry args={[0.07, 9, 7]} />
        <Mat color={NOSE} />
      </mesh>
      <mesh position={[0, -0.15, 0.2]} scale={[1.3, 0.6, 0.7]}>
        <sphereGeometry args={[0.05, 8, 6]} />
        <Mat color={FLUFF} />
      </mesh>
      <DotEyes eyes={eyes} y={0.04} x={0.12} z={0.215} r={0.036} />
      <Blush y={-0.06} x={0.18} z={0.18} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.1, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, -0.04, 0.15]} scale={[0.72, 0.82, 0.45]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color={FLUFF} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={PAW} />,
  Foot: () => (
    <mesh scale={[1.05, 0.75, 1.25]}>
      <sphereGeometry args={[0.09, 8, 6]} />
      <Mat color={PAW} />
    </mesh>
  ),
};
