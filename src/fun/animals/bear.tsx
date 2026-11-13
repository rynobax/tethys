import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#8a5a3b";
const TAN = "#d9b38c";
const PAW = "#5e3b26";
const NOSE = "#2a1d17";

/** A brown bear: a broad head, a short tan snout, and little round ears. */
export const bear: AnimalDesign = {
  coat: COAT,
  accent: "#f0a830",
  top: 1.15,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.12, 0.97, 1]}>
        <sphereGeometry args={[0.26, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* A rounded snout, a broad dark nose. */}
      <mesh position={[0, -0.07, 0.19]} scale={[1.2, 0.85, 0.9]}>
        <sphereGeometry args={[0.11, 10, 8]} />
        <Mat color={TAN} />
      </mesh>
      <mesh position={[0, -0.03, 0.29]} scale={[1.35, 0.85, 0.8]}>
        <sphereGeometry args={[0.035, 7, 5]} />
        <Mat color={NOSE} />
      </mesh>
      {/* Small round ears, set wide on the crown, tan inside. */}
      {[-1, 1].map((s) => (
        <group
          key={s}
          position={[s * 0.19, 0.18, -0.01]}
          rotation-z={s * -0.45}
        >
          <mesh scale={[1, 1, 0.6]}>
            <sphereGeometry args={[0.08, 8, 6]} />
            <Mat color={COAT} />
          </mesh>
          <mesh position-z={0.035} scale={[1, 1, 0.45]}>
            <sphereGeometry args={[0.048, 7, 5]} />
            <Mat color={TAN} />
          </mesh>
        </group>
      ))}
      <DotEyes eyes={eyes} y={0.04} x={0.1} z={0.225} r={0.04} />
      <Blush y={-0.05} x={0.18} z={0.19} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.1, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, -0.05, 0.15]} scale={[0.78, 0.82, 0.45]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color={TAN} />
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
  Tail: () => (
    <mesh position-z={-0.02} scale={[1, 0.9, 0.8]}>
      <sphereGeometry args={[0.06, 7, 5]} />
      <Mat color={COAT} />
    </mesh>
  ),
};
