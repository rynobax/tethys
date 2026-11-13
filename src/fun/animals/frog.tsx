import { Blush, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const SKIN = "#6fb84e";
const PALE = "#e3efb0";
const LIP = "#2f5a26";

export const frog: AnimalDesign = {
  coat: SKIN,
  accent: "#ff6f61",
  top: 1.1,
  Head: ({ eyes }) => (
    <group position-y={-0.03}>
      {/* A wide, flat head with a pale chin. */}
      <mesh scale={[1.3, 0.66, 1.02]}>
        <sphereGeometry args={[0.26, 12, 10]} />
        <Mat color={SKIN} />
      </mesh>
      <mesh position={[0, -0.07, 0.08]} scale={[1.35, 0.5, 1]}>
        <sphereGeometry args={[0.17, 10, 6]} />
        <Mat color={PALE} />
      </mesh>
      {/* The wide mouth, wrapping round the front. */}
      <group position-y={-0.045} scale={[1.28, 1, 1]}>
        <mesh rotation={[Math.PI / 2, 0, Math.PI / 2 - 0.9]}>
          <torusGeometry args={[0.243, 0.013, 3, 14, 1.8]} />
          <Mat color={LIP} />
        </mesh>
      </group>
      {/* The lids the eyes bulge from, on top of the head. */}
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.15, 0.12, 0.05]}>
          <sphereGeometry args={[0.095, 9, 7]} />
          <Mat color={SKIN} />
        </mesh>
      ))}
      <group ref={eyes} position-y={0.15}>
        {[-1, 1].map((s) => (
          <group key={s} position={[s * 0.15, 0, 0.085]}>
            <mesh scale={[1, 1, 0.8]}>
              <sphereGeometry args={[0.075, 9, 7]} />
              <Mat color="#fbf3c8" />
            </mesh>
            <mesh position-z={0.045} scale={[1, 1.1, 0.6]}>
              <sphereGeometry args={[0.048, 8, 6]} />
              <Mat color="#1d1d1f" />
            </mesh>
            <mesh position={[0.016, 0.02, 0.07]}>
              <sphereGeometry args={[0.014, 5, 4]} />
              <meshBasicMaterial color="#ffffff" />
            </mesh>
          </group>
        ))}
      </group>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.045, 0.035, 0.252]}>
          <sphereGeometry args={[0.012, 5, 4]} />
          <Mat color={LIP} />
        </mesh>
      ))}
      <Blush y={-0.01} x={0.17} z={0.21} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.08, 1.1, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={SKIN} />
      </mesh>
      <mesh position={[0, -0.03, 0.145]} scale={[0.8, 0.95, 0.5]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color={PALE} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={SKIN} />,
  Foot: () => (
    <>
      <mesh scale={[1, 0.7, 1]}>
        <sphereGeometry args={[0.065, 7, 5]} />
        <Mat color={SKIN} />
      </mesh>
      {/* A webbed fan of three toes. */}
      <mesh position={[0, -0.025, -0.02]}>
        <cylinderGeometry args={[0.14, 0.14, 0.025, 3, 1, false, -0.75, 1.5]} />
        <Mat color={SKIN} />
      </mesh>
    </>
  ),
};
