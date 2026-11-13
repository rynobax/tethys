import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#e8833a";
const CREAM = "#fff3e2";
const SOCK = "#4a2f24";

export const fox: AnimalDesign = {
  coat: COAT,
  accent: "#2bb3a3",
  top: 1.3,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.1, 0.95, 1]}>
        <sphereGeometry args={[0.25, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* White cheeks, and the muzzle coming to a point. */}
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.13, -0.09, 0.15]} scale={[1, 0.75, 0.8]}>
          <sphereGeometry args={[0.1, 8, 6]} />
          <Mat color={CREAM} />
        </mesh>
      ))}
      <mesh position={[0, -0.08, 0.26]} rotation-x={Math.PI / 2}>
        <coneGeometry args={[0.11, 0.2, 8]} />
        <Mat color={CREAM} />
      </mesh>
      <mesh position={[0, -0.08, 0.36]}>
        <sphereGeometry args={[0.032, 6, 5]} />
        <Mat color="#1d1d1f" />
      </mesh>
      {/* Tall pointed ears, dark at the tips. */}
      {[-1, 1].map((s) => (
        <group key={s} position={[s * 0.15, 0.18, 0.02]} rotation-z={s * -0.3}>
          <mesh position-y={0.09} scale={[1, 1, 0.5]}>
            <coneGeometry args={[0.11, 0.25, 4]} />
            <Mat color={COAT} />
          </mesh>
          <mesh position-y={0.19} scale={[1, 1, 0.5]}>
            <coneGeometry args={[0.05, 0.08, 4]} />
            <Mat color={SOCK} />
          </mesh>
        </group>
      ))}
      <DotEyes eyes={eyes} y={0.03} x={0.1} z={0.215} />
      <Blush y={-0.04} x={0.17} z={0.19} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, -0.02, 0.13]} scale={[0.75, 1, 0.5]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color={CREAM} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={SOCK} />,
  Foot: () => (
    <mesh scale={[1, 0.75, 1.25]}>
      <sphereGeometry args={[0.085, 8, 6]} />
      <Mat color={SOCK} />
    </mesh>
  ),
  Tail: () => (
    <group rotation-x={-0.95}>
      <mesh position-y={0.17} scale={[1, 2, 1]}>
        <sphereGeometry args={[0.1, 8, 6]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position-y={0.36} scale={[1, 1.2, 1]}>
        <sphereGeometry args={[0.075, 8, 6]} />
        <Mat color={CREAM} />
      </mesh>
    </group>
  ),
};
