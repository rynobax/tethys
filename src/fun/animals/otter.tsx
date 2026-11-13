import { DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#7a4f33";
const CREAM = "#f0dcbc";
const DARK = "#4a3022";
const NOSE = "#2a1d18";

export const otter: AnimalDesign = {
  coat: COAT,
  accent: "#48cae4",
  top: 1.1,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.15, 0.84, 1]}>
        <sphereGeometry args={[0.245, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* A broad pale muzzle, puffed out at the whisker pads. */}
      <mesh position={[0, -0.065, 0.14]} scale={[1.5, 0.95, 1.05]}>
        <sphereGeometry args={[0.15, 10, 8]} />
        <Mat color={CREAM} />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.06, -0.08, 0.24]} scale={[1, 0.8, 0.8]}>
          <sphereGeometry args={[0.065, 8, 6]} />
          <Mat color={CREAM} />
        </mesh>
      ))}
      <mesh position={[0, -0.035, 0.29]} scale={[1.6, 0.9, 0.9]}>
        <sphereGeometry args={[0.035, 7, 5]} />
        <Mat color={NOSE} />
      </mesh>
      {/* Whiskers fanning out from the pads. */}
      {[-1, 1].flatMap((s) =>
        [0.12, -0.12].map((tilt) => (
          <mesh
            key={`${s}${tilt}`}
            position={[s * 0.17, -0.08, 0.24]}
            rotation={[0, s * 0.35, Math.PI / 2 + s * tilt]}
          >
            <cylinderGeometry args={[0.005, 0.005, 0.16, 3]} />
            <meshBasicMaterial color="#fffaf0" />
          </mesh>
        )),
      )}
      {/* Small round ears, low and wide on the head. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.235, 0.08, 0.0]}
          rotation-z={s * -0.6}
          scale={[1, 1, 0.55]}
        >
          <sphereGeometry args={[0.05, 7, 5]} />
          <Mat color={DARK} />
        </mesh>
      ))}
      <DotEyes eyes={eyes} y={0.04} x={0.1} z={0.215} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[0.92, 1.2, 0.92]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, 0.08, 0.135]} scale={[0.7, 0.9, 0.42]}>
        <sphereGeometry args={[0.2, 14, 12]} />
        <Mat color={CREAM} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={DARK} />,
  Foot: () => (
    <mesh scale={[1.2, 0.6, 1.3]}>
      <sphereGeometry args={[0.08, 8, 6]} />
      <Mat color={DARK} />
    </mesh>
  ),
  Tail: () => (
    <group rotation-x={-2.0}>
      <mesh position-y={0.12}>
        <cylinderGeometry args={[0.055, 0.085, 0.26, 8]} />
        <Mat color={COAT} />
      </mesh>
      <group position-y={0.25} rotation-x={0.35}>
        <mesh position-y={0.1}>
          <coneGeometry args={[0.055, 0.22, 8]} />
          <Mat color={COAT} />
        </mesh>
      </group>
    </group>
  ),
};
