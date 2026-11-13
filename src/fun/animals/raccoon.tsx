import { DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#8d9096";
const PALE = "#ecebe6";
const MASK = "#2b2b30";

/** A raccoon: grey, with a black bandit mask, a pointed muzzle and a ringed tail. */
export const raccoon: AnimalDesign = {
  coat: COAT,
  accent: "#9b5de5",
  top: 1.18,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.12, 0.95, 1]}>
        <sphereGeometry args={[0.25, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* White brows over the mask, which meets across the bridge of the nose. */}
      {[-1, 1].map((s) => (
        <group key={s}>
          <mesh
            position={[s * 0.1, 0.115, 0.19]}
            rotation={[-0.45, s * 0.45, s * -0.15]}
            scale={[1.3, 0.6, 0.4]}
          >
            <sphereGeometry args={[0.055, 8, 6]} />
            <Mat color={PALE} />
          </mesh>
          <mesh
            position={[s * 0.11, 0.03, 0.2]}
            rotation={[0, s * 0.5, s * -0.2]}
            scale={[1.55, 0.8, 0.5]}
          >
            <sphereGeometry args={[0.075, 8, 6]} />
            <Mat color={MASK} />
          </mesh>
        </group>
      ))}
      {/* A pale, pointed muzzle with a black nose at its tip. */}
      <mesh position={[0, -0.075, 0.22]} scale={[0.95, 0.75, 1.35]}>
        <sphereGeometry args={[0.09, 8, 6]} />
        <Mat color={PALE} />
      </mesh>
      <mesh position={[0, -0.06, 0.34]}>
        <sphereGeometry args={[0.03, 6, 5]} />
        <Mat color={MASK} />
      </mesh>
      {/* Rounded, upright ears, edged in white. */}
      {[-1, 1].map((s) => (
        <group key={s} position={[s * 0.15, 0.18, 0]} rotation-z={s * -0.35}>
          <mesh position-y={0.05} scale={[1, 1, 0.45]}>
            <coneGeometry args={[0.1, 0.15, 6]} />
            <Mat color={PALE} />
          </mesh>
          <mesh position={[0, 0.04, 0.015]} scale={[1, 1, 0.45]}>
            <coneGeometry args={[0.072, 0.11, 6]} />
            <Mat color={MASK} />
          </mesh>
        </group>
      ))}
      <DotEyes
        eyes={eyes}
        y={0.035}
        x={0.11}
        z={0.235}
        r={0.032}
        color="#0c0c0e"
      />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.05, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, -0.04, 0.15]} scale={[0.72, 0.82, 0.45]}>
        <sphereGeometry args={[0.2, 10, 8]} />
        <Mat color="#c9c8c4" />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={MASK} />,
  Foot: () => (
    <mesh scale={[1, 0.75, 1.25]}>
      <sphereGeometry args={[0.085, 8, 6]} />
      <Mat color={MASK} />
    </mesh>
  ),
  /** Bushy, in alternating grey and black rings to a black tip. */
  Tail: () => (
    <group rotation-x={-1.25}>
      {[0.09, 0.19, 0.29, 0.39].map((y, k) => (
        <mesh key={y} position-y={y} scale={[1, 0.85, 1]}>
          <sphereGeometry args={[0.1 - k * 0.008, 8, 6]} />
          <Mat color={k % 2 ? MASK : COAT} />
        </mesh>
      ))}
    </group>
  ),
};
