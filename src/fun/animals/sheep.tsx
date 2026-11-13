import { Blush, Mat, RIG, type AnimalDesign } from "./rig";

const WOOL = "#f7f2e6";
const FACE = "#433733";
const HOOF = "#2f2724";

/** Wool lumps over the shoulders and back, around the body's fleece. */
const PUFFS: [number, number, number, number][] = [
  [-0.17, 0.5, -0.02, 0.13],
  [0.17, 0.5, -0.02, 0.13],
  [0, 0.42, -0.14, 0.15],
];

export const sheep: AnimalDesign = {
  coat: WOOL,
  accent: "#4d8dff",
  top: 1.2,
  Head: ({ eyes }) => (
    <group>
      {/* A dark face under a cap of wool. */}
      <mesh position-z={0.02} scale={[0.95, 1, 1.05]}>
        <sphereGeometry args={[0.22, 12, 10]} />
        <Mat color={FACE} />
      </mesh>
      <mesh position={[0, 0.13, -0.04]}>
        <icosahedronGeometry args={[0.19, 1]} />
        <Mat color={WOOL} />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.13, 0.08, -0.07]}>
          <icosahedronGeometry args={[0.14, 1]} />
          <Mat color={WOOL} />
        </mesh>
      ))}
      {/* Ears flopped out sideways. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.24, -0.01, 0.02]}
          rotation={[0, 0, s * -1.35]}
          scale={[0.55, 1.3, 0.3]}
        >
          <sphereGeometry args={[0.085, 8, 6]} />
          <Mat color={FACE} />
        </mesh>
      ))}
      <mesh position={[0, -0.1, 0.235]} scale={[1.4, 0.8, 0.6]}>
        <sphereGeometry args={[0.03, 6, 5]} />
        <Mat color="#d98b95" />
      </mesh>
      {/* Eyes with whites, so they show on the dark face. */}
      <group ref={eyes} position-y={0}>
        {[-1, 1].map((s) => (
          <group key={s} position={[s * 0.085, 0, 0.2]}>
            <mesh scale={[1, 1.1, 0.5]}>
              <sphereGeometry args={[0.052, 8, 6]} />
              <Mat color="#ffffff" />
            </mesh>
            <mesh position={[s * -0.008, 0, 0.022]} scale={[1, 1.15, 0.6]}>
              <sphereGeometry args={[0.032, 8, 6]} />
              <Mat color="#1d1d1f" />
            </mesh>
          </group>
        ))}
      </group>
      <Blush y={-0.07} x={0.14} z={0.18} />
    </group>
  ),
  Torso: () => (
    <group>
      <mesh position-y={(RIG.hip + RIG.chest) / 2} scale={[1, 1.05, 0.85]}>
        <icosahedronGeometry args={[0.27, 1]} />
        <Mat color={WOOL} />
      </mesh>
      {PUFFS.map(([x, y, z, r], i) => (
        <mesh key={i} position={[x, y, z]}>
          <icosahedronGeometry args={[r, 1]} />
          <Mat color={WOOL} />
        </mesh>
      ))}
    </group>
  ),
  Arm: () => (
    <>
      <mesh position-y={-0.14}>
        <capsuleGeometry args={[0.05, 0.16, 3, 6]} />
        <Mat color={FACE} />
      </mesh>
      <mesh position-y={-RIG.hand + 0.03}>
        <sphereGeometry args={[0.06, 7, 5]} />
        <Mat color={HOOF} />
      </mesh>
    </>
  ),
  Foot: () => (
    <mesh scale={[0.95, 0.8, 1.2]}>
      <sphereGeometry args={[0.08, 8, 6]} />
      <Mat color={HOOF} />
    </mesh>
  ),
  Tail: () => (
    <mesh position={[0, 0.03, -0.06]}>
      <icosahedronGeometry args={[0.08, 1]} />
      <Mat color={WOOL} />
    </mesh>
  ),
};
