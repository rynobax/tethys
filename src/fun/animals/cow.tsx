import { DotEyes, Mat, RIG, type AnimalDesign } from "./rig";

const COAT = "#f6f3ee";
const PATCH = "#2b2a2c";
const PINK = "#f3b4b6";
const HORN = "#efe2c0";
const HOOF = "#4a3f3d";

/** A black patch lying on a sphere of radius `r`, `yaw` round from the front. */
function Patch({
  r,
  y,
  yaw,
  size,
}: {
  r: number;
  y: number;
  yaw: number;
  size: number;
}) {
  const out = Math.sqrt(r * r - y * y) - size * 0.2;
  return (
    <mesh
      position={[Math.sin(yaw) * out, y, Math.cos(yaw) * out]}
      rotation-y={yaw}
      scale={[1.2, 1, 0.4]}
    >
      <sphereGeometry args={[size, 8, 6]} />
      <Mat color={PATCH} />
    </mesh>
  );
}

export const cow: AnimalDesign = {
  coat: COAT,
  accent: "#e5677f",
  top: 1.2,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.05, 1, 1]}>
        <sphereGeometry args={[0.25, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <Patch r={0.25} y={0.15} yaw={-0.8} size={0.1} />
      {/* A wide pink muzzle with two nostrils. */}
      <mesh position={[0, -0.1, 0.17]} scale={[1.25, 0.8, 0.85]}>
        <sphereGeometry args={[0.14, 10, 8]} />
        <Mat color={PINK} />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.055, -0.08, 0.285]}>
          <sphereGeometry args={[0.02, 5, 4]} />
          <Mat color="#9c5a5e" />
        </mesh>
      ))}
      {/* Ears out to the sides, the left one black, and two stubby horns. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.28, 0.06, -0.02]}
          rotation-z={s * -0.35}
          scale={[1.5, 0.55, 0.6]}
        >
          <sphereGeometry args={[0.08, 8, 6]} />
          <Mat color={s < 0 ? PATCH : COAT} />
        </mesh>
      ))}
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.13, 0.25, -0.01]} rotation-z={s * -0.55}>
          <coneGeometry args={[0.042, 0.14, 6]} />
          <Mat color={HORN} />
        </mesh>
      ))}
      <DotEyes eyes={eyes} y={0.06} x={0.11} z={0.2} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <group scale={[1, 1.15, 0.95]}>
        <mesh>
          <sphereGeometry args={[0.23, 12, 10]} />
          <Mat color={COAT} />
        </mesh>
        <Patch r={0.23} y={0.06} yaw={2.3} size={0.12} />
        <Patch r={0.23} y={-0.07} yaw={-1.3} size={0.09} />
      </group>
    </group>
  ),
  Arm: () => (
    <>
      <mesh position-y={-0.14}>
        <capsuleGeometry args={[0.06, 0.16, 3, 6]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position-y={-RIG.hand + 0.03}>
        <sphereGeometry args={[0.066, 7, 5]} />
        <Mat color={HOOF} />
      </mesh>
    </>
  ),
  Foot: () => (
    <mesh scale={[1, 0.8, 1.15]}>
      <cylinderGeometry args={[0.07, 0.085, 0.1, 8]} />
      <Mat color={HOOF} />
    </mesh>
  ),
  Tail: () => (
    <group rotation-x={-2.45}>
      <mesh position-y={0.11}>
        <capsuleGeometry args={[0.02, 0.18, 3, 5]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position-y={0.23} scale={[1, 1.4, 1]}>
        <sphereGeometry args={[0.045, 6, 5]} />
        <Mat color={PATCH} />
      </mesh>
    </group>
  ),
};
