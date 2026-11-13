import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#9aa6ba";
const PALE = "#b7c0cf";
const EAR = "#e9b3b8";
const IVORY = "#fbf3e0";
const TUFT = "#4b4f5c";

/** The trunk's segments, root to tip: length, radius, and its bend. */
const TRUNK = [
  { l: 0.1, r: 0.075, bend: -0.3 },
  { l: 0.09, r: 0.062, bend: -0.05 },
  { l: 0.08, r: 0.052, bend: -0.4 },
  { l: 0.06, r: 0.045, bend: -0.65 },
];

/** Each segment hangs from the end of the one before, curling forward. */
function Trunk({ i = 0 }: { i?: number }) {
  const seg = TRUNK[i];
  if (!seg) return null;
  const next = TRUNK[i + 1];
  return (
    <group rotation-x={seg.bend}>
      <mesh position-y={-seg.l / 2}>
        <cylinderGeometry
          args={[seg.r, next?.r ?? seg.r * 0.85, seg.l + 0.02, 7]}
        />
        <Mat color={COAT} />
      </mesh>
      <group position-y={-seg.l}>
        <Trunk i={i + 1} />
      </group>
    </group>
  );
}

export const elephant: AnimalDesign = {
  coat: COAT,
  accent: "#ff9f1c",
  top: 1.16,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.1, 0.98, 1]}>
        <sphereGeometry args={[0.26, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      {/* Big flat fan ears, pink inside, swept out to the sides. */}
      {[-1, 1].map((s) => (
        <group
          key={s}
          position={[s * 0.27, 0.0, -0.04]}
          rotation={[0, s * 0.55, s * -0.15]}
        >
          <mesh
            position-x={s * 0.11}
            rotation-x={Math.PI / 2}
            scale={[1, 1, 1.15]}
          >
            <cylinderGeometry args={[0.21, 0.21, 0.035, 9]} />
            <Mat color={COAT} />
          </mesh>
          <mesh
            position={[s * 0.11, -0.01, 0.015]}
            rotation-x={Math.PI / 2}
            scale={[1, 1, 1.15]}
          >
            <cylinderGeometry args={[0.145, 0.145, 0.02, 9]} />
            <Mat color={EAR} />
          </mesh>
        </group>
      ))}
      <group position={[0, -0.02, 0.21]}>
        <Trunk />
      </group>
      {/* Little tusks either side of the trunk. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.09, -0.14, 0.19]}
          rotation={[1.9, 0, s * 0.2]}
        >
          <coneGeometry args={[0.028, 0.11, 5]} />
          <Mat color={IVORY} />
        </mesh>
      ))}
      <DotEyes eyes={eyes} y={0.04} x={0.1} z={0.225} />
      <Blush y={-0.05} x={0.17} z={0.2} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1.05, 1.15, 0.98]}>
        <sphereGeometry args={[0.235, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={PALE} />,
  Foot: () => (
    <mesh position-y={0.005}>
      <cylinderGeometry args={[0.08, 0.09, 0.1, 8]} />
      <Mat color={COAT} />
    </mesh>
  ),
  Tail: () => (
    <group rotation-x={-2.6}>
      <mesh position-y={0.08}>
        <cylinderGeometry args={[0.014, 0.022, 0.16, 5]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position-y={0.19} rotation-x={Math.PI}>
        <coneGeometry args={[0.035, 0.08, 5]} />
        <Mat color={TUFT} />
      </mesh>
    </group>
  ),
};
