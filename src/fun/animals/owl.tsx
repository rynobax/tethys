import { Mat, RIG, type AnimalDesign } from "./rig";

const FEATHER = "#9a6b45";
const DARK = "#5e3f2a";
const DISC = "#ecd9b4";
const BEAK = "#d9a441";
const WING = "#85593a";

export const owl: AnimalDesign = {
  coat: FEATHER,
  accent: "#6fc2ff",
  top: 1.24,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.1, 0.95, 0.95]}>
        <sphereGeometry args={[0.27, 12, 10]} />
        <Mat color={FEATHER} />
      </mesh>
      {/* The facial disc, a pale ring round each eye. */}
      {[-1, 1].map((s) => (
        <group key={s} position={[s * 0.1, 0.01, 0.205]} rotation-y={s * 0.35}>
          <mesh rotation-x={Math.PI / 2}>
            <cylinderGeometry args={[0.135, 0.135, 0.05, 10]} />
            <Mat color={DISC} />
          </mesh>
        </group>
      ))}
      <group ref={eyes} position-y={0.01}>
        {[-1, 1].map((s) => (
          <group key={s} position={[s * 0.1, 0, 0.235]} rotation-y={s * 0.35}>
            <mesh scale={[1, 1, 0.45]}>
              <sphereGeometry args={[0.078, 9, 7]} />
              <Mat color="#f5b62a" />
            </mesh>
            <mesh position-z={0.025} scale={[1, 1, 0.5]}>
              <sphereGeometry args={[0.05, 8, 6]} />
              <Mat color="#1d1d1f" />
            </mesh>
            <mesh position={[0.018, 0.022, 0.045]}>
              <sphereGeometry args={[0.016, 5, 4]} />
              <meshBasicMaterial color="#ffffff" />
            </mesh>
          </group>
        ))}
      </group>
      {/* A small beak, hooked down between the eyes. */}
      <mesh position={[0, -0.065, 0.275]} rotation-x={Math.PI - 0.5}>
        <coneGeometry args={[0.045, 0.11, 4]} />
        <Mat color={BEAK} />
      </mesh>
      {/* Ear tufts. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.17, 0.24, 0.02]}
          rotation-z={s * -0.35}
          scale={[1, 1, 0.5]}
        >
          <coneGeometry args={[0.075, 0.21, 4]} />
          <Mat color={DARK} />
        </mesh>
      ))}
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.12, 1.12, 1]}>
        <sphereGeometry args={[0.24, 12, 10]} />
        <Mat color={FEATHER} />
      </mesh>
      <mesh position={[0, -0.03, 0.15]} scale={[0.82, 0.98, 0.5]}>
        <sphereGeometry args={[0.21, 10, 8]} />
        <Mat color={DISC} />
      </mesh>
      {/* Chevron speckles down the breast. */}
      {[
        [-0.075, 0.06, 0.238],
        [0, 0.07, 0.245],
        [0.075, 0.06, 0.238],
        [-0.04, -0.03, 0.257],
        [0.04, -0.03, 0.257],
      ].map(([x, y, z]) => (
        <mesh
          key={`${x},${y}`}
          position={[x, y, z]}
          rotation-x={Math.PI}
          scale={[1, 1, 0.3]}
        >
          <coneGeometry args={[0.026, 0.035, 3]} />
          <Mat color={DARK} />
        </mesh>
      ))}
    </group>
  ),
  Arm: ({ side }) => (
    <mesh position-y={-0.14} rotation-z={side * 0.1} scale={[0.38, 1.65, 0.9]}>
      <sphereGeometry args={[0.1, 8, 6]} />
      <Mat color={WING} />
    </mesh>
  ),
  Foot: () => (
    <>
      {/* Three hooked talons fanned forward. */}
      {[-0.45, 0, 0.45].map((a) => (
        <group key={a} rotation-y={a}>
          <mesh
            position={[0, -0.02, 0.05]}
            rotation-x={Math.PI / 2}
            scale={[1, 1, 0.7]}
          >
            <coneGeometry args={[0.03, 0.12, 4]} />
            <Mat color={BEAK} />
          </mesh>
        </group>
      ))}
    </>
  ),
  Tail: () => (
    <mesh
      position={[0, 0.02, -0.06]}
      rotation-x={-Math.PI / 2 - 0.4}
      scale={[1, 1, 0.35]}
    >
      <coneGeometry args={[0.09, 0.16, 4]} />
      <Mat color={DARK} />
    </mesh>
  ),
};
