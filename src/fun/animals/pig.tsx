import { Blush, DotEyes, Mat, RIG, type AnimalDesign } from "./rig";

const SKIN = "#f4b0bd";
const SNOUT = "#ee95a6";
const BELLY = "#fbd3da";
const HOOF = "#b8707c";
const EAR = "#f2a2b2";

export const pig: AnimalDesign = {
  coat: SKIN,
  accent: "#4cc38a",
  top: 1.16,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.1, 0.95, 1]}>
        <sphereGeometry args={[0.26, 12, 10]} />
        <Mat color={SKIN} />
      </mesh>
      {/* The flat snout disc, with its two nostrils. */}
      <mesh position={[0, -0.05, 0.245]} rotation-x={Math.PI / 2}>
        <cylinderGeometry args={[0.1, 0.11, 0.09, 10]} />
        <Mat color={SNOUT} />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.036, -0.05, 0.29]} scale={[0.7, 1, 0.4]}>
          <sphereGeometry args={[0.024, 6, 4]} />
          <Mat color="#8a3f50" />
        </mesh>
      ))}
      {/* Ears flopping forward and down over the brow. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.19, 0.175, 0.11]}
          rotation={[-1.13, s * -0.19, s * -2.67]}
          scale={[1.2, 1, 0.3]}
        >
          <coneGeometry args={[0.12, 0.22, 3]} />
          <Mat color={EAR} />
        </mesh>
      ))}
      <DotEyes eyes={eyes} y={0.07} x={0.1} z={0.225} r={0.038} />
      <Blush y={-0.04} x={0.18} z={0.19} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.08, 1.1, 0.98]}>
        <sphereGeometry args={[0.24, 12, 10]} />
        <Mat color={SKIN} />
      </mesh>
      <mesh position={[0, -0.03, 0.15]} scale={[0.8, 0.95, 0.5]}>
        <sphereGeometry args={[0.21, 10, 8]} />
        <Mat color={BELLY} />
      </mesh>
    </group>
  ),
  Arm: () => (
    <>
      <mesh position-y={-0.14}>
        <capsuleGeometry args={[0.065, 0.15, 3, 6]} />
        <Mat color={SKIN} />
      </mesh>
      <mesh position-y={-RIG.hand + 0.03} scale={[1, 0.9, 1]}>
        <sphereGeometry args={[0.07, 7, 5]} />
        <Mat color={HOOF} />
      </mesh>
    </>
  ),
  Foot: () => (
    <>
      <mesh scale={[1, 0.8, 1.15]}>
        <sphereGeometry args={[0.085, 8, 6]} />
        <Mat color={SKIN} />
      </mesh>
      <mesh position={[0, -0.02, 0.05]} scale={[1, 0.6, 0.9]}>
        <sphereGeometry args={[0.07, 7, 5]} />
        <Mat color={HOOF} />
      </mesh>
    </>
  ),
  Tail: () => (
    <mesh position={[0, 0.05, -0.06]} rotation-y={Math.PI / 4}>
      <torusGeometry args={[0.05, 0.02, 4, 12, Math.PI * 1.7]} />
      <Mat color={SKIN} />
    </mesh>
  ),
};
