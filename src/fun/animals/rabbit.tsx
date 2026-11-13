import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#d9b48f";
const CREAM = "#fff6ea";
const PINK = "#f4a3b4";

export const rabbit: AnimalDesign = {
  coat: COAT,
  accent: "#7a6cf0",
  top: 1.5,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.05, 0.95, 1]}>
        <sphereGeometry args={[0.25, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* Long ears, pink inside: the left stands straight, the right flops out. */}
      {[-1, 1].map((s) => (
        <group
          key={s}
          position={[s * 0.09, 0.17, -0.03]}
          rotation={[-0.15, 0, s === 1 ? -0.55 : 0.12]}
        >
          <mesh position-y={0.2} scale={[1, 1, 0.45]}>
            <capsuleGeometry args={[0.065, 0.3, 3, 6]} />
            <Mat color={COAT} />
          </mesh>
          <mesh position={[0, 0.2, 0.022]} scale={[1, 1, 0.3]}>
            <capsuleGeometry args={[0.038, 0.24, 3, 6]} />
            <Mat color={PINK} />
          </mesh>
        </group>
      ))}
      {/* Puffed cheeks, a pink button nose, and two front teeth. */}
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.05, -0.085, 0.2]}>
          <sphereGeometry args={[0.068, 8, 6]} />
          <Mat color={CREAM} />
        </mesh>
      ))}
      <mesh position={[0, -0.04, 0.262]} scale={[1.3, 0.85, 0.8]}>
        <sphereGeometry args={[0.03, 6, 5]} />
        <Mat color="#e8708a" />
      </mesh>
      <mesh position={[0, -0.13, 0.245]}>
        <boxGeometry args={[0.045, 0.04, 0.02]} />
        <Mat color="#ffffff" />
      </mesh>
      <DotEyes eyes={eyes} y={0.04} x={0.1} z={0.215} />
      <Blush y={-0.04} x={0.165} z={0.19} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, -0.03, 0.13]} scale={[0.75, 1, 0.5]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color={CREAM} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={CREAM} />,
  /** Long hind feet, the rabbit's tell from above. */
  Foot: () => (
    <mesh position-z={0.03} scale={[0.9, 0.65, 1.7]}>
      <sphereGeometry args={[0.085, 8, 6]} />
      <Mat color={CREAM} />
    </mesh>
  ),
  Tail: () => (
    <mesh position={[0, 0.02, -0.05]}>
      <sphereGeometry args={[0.085, 7, 5]} />
      <Mat color="#ffffff" />
    </mesh>
  ),
};
