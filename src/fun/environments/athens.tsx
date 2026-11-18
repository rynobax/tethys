import { Mat } from "../animals/rig";
import type { Palette } from "../palette";

const MARBLE = "#f4efe3";
const SHADE = "#ddd5c3";
const TERRACOTTA = "#c0603f";
const BLACK_FIGURE = "#2b211b";
const LAUREL = "#6f9a3c";

/** A fluted column on a base, a square capital on top. */
export function Column({
  height,
  radius = 0.09,
  color = MARBLE,
}: {
  height: number;
  radius?: number;
  color?: string;
}) {
  return (
    <group>
      <mesh position-y={0.03}>
        <boxGeometry args={[radius * 2.6, 0.06, radius * 2.6]} />
        <Mat color={color} />
      </mesh>
      <mesh position-y={height / 2}>
        <cylinderGeometry args={[radius * 0.9, radius, height - 0.12, 10]} />
        <Mat color={color} />
      </mesh>
      <mesh position-y={height - 0.03}>
        <boxGeometry args={[radius * 2.6, 0.06, radius * 2.6]} />
        <Mat color={color} />
      </mesh>
    </group>
  );
}

/** Along the back: cypresses and olives, and a column left standing. */
export function AthensBackdrop({
  x,
  z,
  s,
  c,
  palette,
}: {
  x: number;
  z: number;
  s: number;
  c: number;
  palette: Palette;
}) {
  if (c === 1) {
    return (
      <group position={[x, 0, z]} scale={s}>
        <mesh position-y={0.2} rotation-z={0.15}>
          <cylinderGeometry args={[0.05, 0.09, 0.42, 5]} />
          <Mat color={palette.trunk} />
        </mesh>
        {[
          [0, 0.55, 0, 0.3],
          [-0.2, 0.45, 0.05, 0.22],
          [0.2, 0.5, -0.05, 0.24],
        ].map(([px, py, pz, r], i) => (
          <mesh key={i} position={[px, py, pz]} scale={[1, 0.75, 1]}>
            <dodecahedronGeometry args={[r, 0]} />
            <Mat color={palette.foliage[1]} />
          </mesh>
        ))}
      </group>
    );
  }
  if (c === 2 && Math.floor(Math.abs(x) * 5.7) % 3 === 0) {
    return (
      <group position={[x, 0, z]} scale={s}>
        <Column height={1.1 + (Math.abs(x * 13) % 0.5)} radius={0.1} />
      </group>
    );
  }
  return (
    <group position={[x, 0, z]} scale={s}>
      <mesh position-y={0.08}>
        <cylinderGeometry args={[0.04, 0.05, 0.16, 5]} />
        <Mat color={palette.trunk} />
      </mesh>
      <mesh position-y={0.68} scale={[1, 1, 1]}>
        <coneGeometry args={[0.2, 1.2, 7]} />
        <Mat color={palette.foliage[c === 2 ? 2 : 0]} />
      </mesh>
    </group>
  );
}

/** A drum of a fallen column, or an amphora set down. */
export function Ruin({
  x,
  z,
  s,
  r,
  palette,
}: {
  x: number;
  z: number;
  s: number;
  r: number;
  palette: Palette;
}) {
  if (Math.floor(r * 10) % 3 === 0) return <Amphora x={x} z={z} s={s} />;
  return (
    <mesh
      position={[x, s * 0.5, z]}
      rotation={[0, r, Math.floor(r * 7) % 2 ? Math.PI / 2 : 0]}
    >
      <cylinderGeometry args={[s * 0.75, s * 0.75, s, 10]} />
      <Mat color={palette.rock} />
    </mesh>
  );
}

function Amphora({ x, z, s }: { x: number; z: number; s: number }) {
  return (
    <group position={[x, 0, z]} scale={s * 2.2}>
      <mesh position-y={0.17} scale={[1, 1.4, 1]}>
        <sphereGeometry args={[0.11, 10, 8]} />
        <Mat color={TERRACOTTA} />
      </mesh>
      <mesh position-y={0.17} scale={[1, 0.25, 1]}>
        <sphereGeometry args={[0.112, 10, 4]} />
        <Mat color={BLACK_FIGURE} />
      </mesh>
      <mesh position-y={0.34}>
        <cylinderGeometry args={[0.04, 0.035, 0.1, 8]} />
        <Mat color={TERRACOTTA} />
      </mesh>
      {[-1, 1].map((side) => (
        <mesh key={side} position={[side * 0.06, 0.32, 0]} rotation-y={0}>
          <torusGeometry args={[0.035, 0.008, 4, 8, Math.PI]} />
          <Mat color={TERRACOTTA} />
        </mesh>
      ))}
    </group>
  );
}

/** The portal's frame: two columns and a lintel. */
export function AthensGate() {
  return (
    <>
      {[-1, 1].map((s) => (
        <group key={s} position-x={s * 0.6}>
          <Column height={1.3} radius={0.1} />
        </group>
      ))}
      <mesh position-y={1.36}>
        <boxGeometry args={[1.5, 0.14, 0.3]} />
        <Mat color={MARBLE} />
      </mesh>
      <mesh position-y={1.55} rotation-x={-Math.PI / 2} scale={[1, 1, 0.3]}>
        <cylinderGeometry args={[0.8, 0.8, 0.28, 3]} />
        <Mat color={MARBLE} />
      </mesh>
    </>
  );
}

/** A tablet on an easel, chalked on. */
export function Tablet() {
  return (
    <>
      {[-0.32, 0.32].map((x) => (
        <mesh key={x} position={[x, 0.4, -0.04]} rotation-z={x * -0.2}>
          <cylinderGeometry args={[0.022, 0.026, 0.84, 5]} />
          <Mat color="#8a6a4a" />
        </mesh>
      ))}
      <mesh position-y={0.62}>
        <boxGeometry args={[0.74, 0.5, 0.04]} />
        <Mat color="#8a6a4a" />
      </mesh>
      <mesh position={[0, 0.62, 0.021]}>
        <planeGeometry args={[0.66, 0.42]} />
        <meshStandardMaterial color="#3b3a36" />
      </mesh>
      {[0.12, 0.03, -0.06, -0.14].map((y, j) => (
        <mesh key={y} position={[-0.05 + (j % 2) * 0.05, 0.62 + y, 0.024]}>
          <planeGeometry args={[0.46 - j * 0.07, 0.018]} />
          <meshBasicMaterial color="#efe8d6" />
        </mesh>
      ))}
    </>
  );
}

/** Where tickets are sold: a little temple, its window between columns. */
export function TempleKiosk() {
  return (
    <group>
      <mesh position-y={0.04}>
        <boxGeometry args={[0.86, 0.08, 0.68]} />
        <Mat color={SHADE} />
      </mesh>
      <mesh position={[0, 0.44, -0.04]}>
        <boxGeometry args={[0.62, 0.72, 0.46]} />
        <Mat color={MARBLE} />
      </mesh>
      <mesh position={[0, 0.5, 0.191]}>
        <planeGeometry args={[0.34, 0.26]} />
        <meshBasicMaterial color="#3a2a20" />
      </mesh>
      {[-0.34, 0.34].map((x) => (
        <group key={x} position={[x, 0.08, 0.24]}>
          <Column height={0.74} radius={0.05} />
        </group>
      ))}
      <mesh position-y={0.86}>
        <boxGeometry args={[0.86, 0.08, 0.66]} />
        <Mat color={MARBLE} />
      </mesh>
      <mesh
        position={[0, 0.98, 0]}
        rotation-x={-Math.PI / 2}
        scale={[1, 1, 0.3]}
      >
        <cylinderGeometry args={[0.5, 0.5, 0.64, 3]} />
        <Mat color={TERRACOTTA} />
      </mesh>
    </group>
  );
}

/** The boat, as a galley: a square sail with a stripe, oars, an eye. */
export function Galley() {
  const hull = "#6b4530";
  return (
    <group>
      <mesh position-y={0.08} scale={[1, 1, 2.3]}>
        <cylinderGeometry args={[0.25, 0.16, 0.18, 10]} />
        <Mat color={hull} />
      </mesh>
      <mesh position-y={0.175} scale={[1, 1, 2.3]}>
        <torusGeometry args={[0.24, 0.025, 4, 14]} />
        <Mat color={TERRACOTTA} />
      </mesh>
      {/* The stern curling up behind. */}
      <mesh position={[0, 0.25, -0.5]} rotation-y={Math.PI / 2}>
        <torusGeometry args={[0.12, 0.035, 4, 8, Math.PI]} />
        <Mat color={hull} />
      </mesh>
      {/* An eye each side of the bow. */}
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.19, 0.12, 0.42]} rotation-y={s * 1.2}>
          <circleGeometry args={[0.035, 8]} />
          <meshBasicMaterial color="#ffffff" />
        </mesh>
      ))}
      {[-1, 1].flatMap((s) =>
        [-0.25, 0, 0.25].map((z) => (
          <mesh
            key={`${s}${z}`}
            position={[s * 0.36, 0.08, z]}
            rotation-z={s * 1.2}
          >
            <cylinderGeometry args={[0.01, 0.012, 0.32, 4]} />
            <Mat color="#c9a46b" />
          </mesh>
        )),
      )}
      <mesh position={[0, 0.65, 0.18]}>
        <cylinderGeometry args={[0.018, 0.018, 1, 5]} />
        <Mat color="#7a5236" />
      </mesh>
      <mesh position={[0, 1.05, 0.18]} rotation-z={Math.PI / 2}>
        <cylinderGeometry args={[0.012, 0.012, 0.7, 4]} />
        <Mat color="#7a5236" />
      </mesh>
      <mesh position={[0, 0.8, 0.2]}>
        <boxGeometry args={[0.62, 0.48, 0.015]} />
        <Mat color="#f6efe0" />
      </mesh>
      <mesh position={[0, 0.8, 0.21]}>
        <boxGeometry args={[0.62, 0.12, 0.015]} />
        <Mat color={TERRACOTTA} />
      </mesh>
    </group>
  );
}

/** A laurel wreath, round the top of the head. */
export function Wreath() {
  const leaves = 14;
  return (
    <group position-y={0.17} rotation-x={-0.25}>
      {Array.from({ length: leaves }, (_, i) => {
        // Open at the front, as a wreath is tied.
        const a = Math.PI / 2 + 0.35 + (i / (leaves - 1)) * (Math.PI * 2 - 0.7);
        return (
          <mesh
            key={i}
            position={[Math.cos(a) * 0.27, 0, Math.sin(a) * 0.27]}
            rotation={[0, -a, 0.6]}
            scale={[1, 0.35, 2]}
          >
            <sphereGeometry args={[0.035, 5, 4]} />
            <Mat color={LAUREL} />
          </mesh>
        );
      })}
    </group>
  );
}

export const ATHENS = { MARBLE, SHADE, TERRACOTTA, LAUREL };
