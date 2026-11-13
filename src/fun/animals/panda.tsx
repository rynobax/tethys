import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const WHITE = "#f4f3ee";
const BLACK = "#2a2a2e";

/** A giant panda: white, with black ears, eye patches, shoulders and legs. */
export const panda: AnimalDesign = {
  coat: WHITE,
  accent: "#6cbf4f",
  top: 1.16,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.12, 0.97, 1]}>
        <sphereGeometry args={[0.26, 12, 10]} />
        <Mat color={WHITE} />
      </mesh>
      {/* Drooping black patches, each eye bright inside its own. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.105, 0.015, 0.215]}
          rotation={[0, s * 0.35, s * 0.55]}
          scale={[0.85, 1.25, 0.45]}
        >
          <sphereGeometry args={[0.07, 8, 6]} />
          <Mat color={BLACK} />
        </mesh>
      ))}
      <mesh position={[0, -0.08, 0.2]} scale={[1.2, 0.8, 0.9]}>
        <sphereGeometry args={[0.085, 10, 8]} />
        <Mat color={WHITE} />
      </mesh>
      <mesh position={[0, -0.05, 0.28]} scale={[1.3, 0.8, 0.8]}>
        <sphereGeometry args={[0.03, 7, 5]} />
        <Mat color={BLACK} />
      </mesh>
      {/* Round black ears on top. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.18, 0.19, -0.01]}
          rotation-z={s * -0.4}
          scale={[1, 1, 0.65]}
        >
          <sphereGeometry args={[0.085, 8, 6]} />
          <Mat color={BLACK} />
        </mesh>
      ))}
      <DotEyes
        eyes={eyes}
        y={0.03}
        x={0.11}
        z={0.245}
        r={0.032}
        color="#0c0c0e"
      />
      <Blush y={-0.07} x={0.18} z={0.18} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.1, 1.12, 0.97]}>
        <sphereGeometry args={[0.24, 12, 10]} />
        <Mat color={WHITE} />
      </mesh>
      {/* The black band over the shoulders, and black legs below. */}
      <mesh position-y={0.15} scale={[1.05, 0.45, 0.88]}>
        <sphereGeometry args={[0.24, 12, 8]} />
        <Mat color={BLACK} />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.12, -0.14, 0.01]} scale={[1, 0.95, 1.1]}>
          <sphereGeometry args={[0.13, 9, 7]} />
          <Mat color={BLACK} />
        </mesh>
      ))}
    </group>
  ),
  Arm: () => <SimpleArm color={BLACK} />,
  Foot: () => (
    <mesh scale={[1.05, 0.75, 1.25]}>
      <sphereGeometry args={[0.09, 8, 6]} />
      <Mat color={BLACK} />
    </mesh>
  ),
  Tail: () => (
    <mesh position-z={-0.02} scale={[1, 0.85, 0.75]}>
      <sphereGeometry args={[0.055, 7, 5]} />
      <Mat color={WHITE} />
    </mesh>
  ),
};
