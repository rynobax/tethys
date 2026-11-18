import { useLayoutEffect, useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { Mat } from "../animals/rig";
import type { Palette } from "../palette";

const STEEL = "#c3c8d0";
const DARK = "#3a3f4a";
const DESK = "#f2efe8";
const POT = "#f1eee8";

/** A plant in a pot, leaves in a loose ball. */
export function PottedPlant({
  x,
  z,
  s,
  palette,
}: {
  x: number;
  z: number;
  s: number;
  palette: Palette;
}) {
  return (
    <group position={[x, 0, z]} scale={s}>
      <mesh position-y={0.14}>
        <cylinderGeometry args={[0.16, 0.12, 0.28, 10]} />
        <Mat color={POT} />
      </mesh>
      <mesh position-y={0.4}>
        <cylinderGeometry args={[0.02, 0.025, 0.3, 4]} />
        <Mat color="#7a5a3c" />
      </mesh>
      {[
        [0, 0.7, 0, 0.24, 0],
        [-0.16, 0.56, 0.06, 0.17, 1],
        [0.17, 0.6, -0.04, 0.18, 2],
        [0.03, 0.9, 0.02, 0.16, 1],
      ].map(([px, py, pz, r, c], i) => (
        <mesh key={i} position={[px, py, pz]}>
          <icosahedronGeometry args={[r, 0]} />
          <Mat color={palette.foliage[c]} />
        </mesh>
      ))}
    </group>
  );
}

/** Along the back: plants in pots, filing cabinets, the water cooler. */
export function OfficeBackdrop({
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
  const pick = Math.floor(Math.abs(x) * 6.1) % 3;
  if (c === 1 && pick === 0) return <Cabinet x={x} z={z} />;
  if (c === 2 && pick === 0) return <WaterCooler x={x} z={z} />;
  if (c === 2 && pick === 1) return <VendingMachine x={x} z={z} />;
  return <PottedPlant x={x} z={z} s={s} palette={palette} />;
}

function Cabinet({ x, z }: { x: number; z: number }) {
  return (
    <group position={[x, 0, z]}>
      <mesh position-y={0.45}>
        <boxGeometry args={[0.42, 0.9, 0.4]} />
        <Mat color={STEEL} />
      </mesh>
      {[0.2, 0.43, 0.66].map((y) => (
        <group key={y} position={[0, y, 0.201]}>
          <mesh>
            <planeGeometry args={[0.36, 0.2]} />
            <meshStandardMaterial color="#b4bac3" />
          </mesh>
          <mesh position={[0, 0.05, 0.01]}>
            <boxGeometry args={[0.1, 0.02, 0.02]} />
            <Mat color={DARK} />
          </mesh>
        </group>
      ))}
    </group>
  );
}

function WaterCooler({ x, z }: { x: number; z: number }) {
  return (
    <group position={[x, 0, z]}>
      <mesh position-y={0.3}>
        <boxGeometry args={[0.3, 0.6, 0.3]} />
        <Mat color={DESK} />
      </mesh>
      <mesh position-y={0.76}>
        <cylinderGeometry args={[0.13, 0.13, 0.32, 10]} />
        <meshStandardMaterial
          color="#8fd3ff"
          transparent
          opacity={0.7}
          roughness={0.1}
        />
      </mesh>
      <mesh position={[0, 0.42, 0.155]}>
        <boxGeometry args={[0.06, 0.04, 0.03]} />
        <Mat color="#3d7bd9" />
      </mesh>
    </group>
  );
}

function VendingMachine({ x, z }: { x: number; z: number }) {
  return (
    <group position={[x, 0, z]}>
      <mesh position-y={0.55}>
        <boxGeometry args={[0.6, 1.1, 0.45]} />
        <Mat color="#d64545" />
      </mesh>
      <mesh position={[-0.06, 0.62, 0.226]}>
        <planeGeometry args={[0.38, 0.72]} />
        <meshBasicMaterial color="#cfe8ff" />
      </mesh>
      {[0.85, 0.65, 0.45].flatMap((y, row) =>
        [-0.18, -0.06, 0.06].map((px, i) => (
          <mesh key={`${y}${px}`} position={[px, y, 0.23]}>
            <planeGeometry args={[0.08, 0.1]} />
            <meshBasicMaterial
              color={["#ffb703", "#3d7bd9", "#7bd96b"][(row + i) % 3]}
            />
          </mesh>
        )),
      )}
    </group>
  );
}

/** A square of carpet tile laid the other way, on the grid. */
export function CarpetTile({
  x,
  z,
  r,
  palette,
}: {
  x: number;
  z: number;
  r: number;
  palette: Palette;
}) {
  const size = 0.6 * Math.max(1, Math.round(r));
  return (
    <mesh
      rotation-x={-Math.PI / 2}
      position={[Math.round(x / 0.6) * 0.6, 0.003, Math.round(z / 0.6) * 0.6]}
    >
      <planeGeometry args={[size, size]} />
      <meshStandardMaterial color={palette.patch} />
    </mesh>
  );
}

/** A box left out, or a bin. */
export function Clutter({
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
  if (Math.floor(r * 10) % 2) {
    return (
      <group position={[x, 0, z]}>
        <mesh position-y={0.11}>
          <cylinderGeometry args={[0.09, 0.07, 0.22, 10, 1, true]} />
          <meshStandardMaterial color={DARK} side={THREE.DoubleSide} />
        </mesh>
        <mesh position-y={0.2}>
          <sphereGeometry args={[0.05, 5, 4]} />
          <Mat color="#ffffff" />
        </mesh>
      </group>
    );
  }
  const w = s * 1.8;
  return (
    <group position={[x, w * 0.35, z]} rotation-y={r}>
      <mesh>
        <boxGeometry args={[w, w * 0.7, w * 0.8]} />
        <Mat color={palette.rock} />
      </mesh>
      <mesh position-y={w * 0.351}>
        <boxGeometry args={[w * 0.18, 0.004, w * 0.81]} />
        <meshBasicMaterial color="#e9d9a8" />
      </mesh>
    </group>
  );
}

/** A sticky note dropped on the carpet; most aren't. */
export function StickyNote({
  x,
  z,
  c,
  j,
  palette,
}: {
  x: number;
  z: number;
  c: number;
  j: number;
  palette: Palette;
}) {
  if (j % 4) return null;
  return (
    <mesh rotation={[-Math.PI / 2, 0, x * 7]} position={[x, 0.005, z]}>
      <planeGeometry args={[0.09, 0.09]} />
      <meshStandardMaterial
        color={palette.flowers[c % palette.flowers.length]}
      />
    </mesh>
  );
}

/** The portal's frame: an elevator, its doors slid open and light inside. */
export function Elevator({ color }: { color: string }) {
  return (
    <>
      <mesh position={[0, 0.7, -0.06]}>
        <boxGeometry args={[1.3, 1.4, 0.1]} />
        <Mat color={STEEL} />
      </mesh>
      <mesh position={[0, 0.62, -0.005]}>
        <planeGeometry args={[0.96, 1.24]} />
        <meshBasicMaterial color="#fffbe8" />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.52, 0.62, 0.005]}>
          <boxGeometry args={[0.1, 1.24, 0.02]} />
          <Mat color="#aab0ba" />
        </mesh>
      ))}
      <mesh position={[0, 1.33, 0.0]}>
        <coneGeometry args={[0.05, 0.07, 3]} />
        <meshBasicMaterial color={color} toneMapped={false} />
      </mesh>
    </>
  );
}

const BALL_COLORS = ["#ff7b7b", "#ffd96a", "#6cbcf7", "#7fe3c0"];

/** The pool, as a ball pit: balls packed in loose, a few bobbing. */
export function BallPit({ w, d }: { w: number; d: number }) {
  const mesh = useRef<THREE.InstancedMesh>(null);
  const balls = useMemo(() => {
    const out: { x: number; y: number; z: number; c: number }[] = [];
    const step = 0.17;
    let i = 0;
    for (let z = -d / 2 + step / 2; z < d / 2; z += step) {
      for (let x = -w / 2 + step / 2; x < w / 2; x += step) {
        i++;
        out.push({
          x: x + Math.sin(i * 12.9) * 0.025,
          y: 0.06 + Math.abs(Math.sin(i * 4.1)) * 0.03,
          z: z + Math.cos(i * 7.3) * 0.025,
          c: Math.floor(Math.abs(Math.sin(i * 91.7)) * BALL_COLORS.length),
        });
      }
    }
    return out;
  }, [w, d]);
  useLayoutEffect(() => {
    const m = mesh.current;
    if (!m) return;
    const matrix = new THREE.Matrix4();
    const color = new THREE.Color();
    balls.forEach((b, i) => {
      m.setMatrixAt(i, matrix.makeTranslation(b.x, b.y, b.z));
      m.setColorAt(i, color.set(BALL_COLORS[b.c]));
    });
    m.instanceMatrix.needsUpdate = true;
    if (m.instanceColor) m.instanceColor.needsUpdate = true;
  }, [balls]);
  return (
    <instancedMesh
      key={balls.length}
      ref={mesh}
      args={[undefined, undefined, balls.length]}
    >
      <sphereGeometry args={[0.085, 8, 6]} />
      <meshStandardMaterial roughness={0.4} />
    </instancedMesh>
  );
}

const BEAN_COLORS = ["#f28c5b", "#5b9cf2", "#9b6bd6", "#5bc48e"];

/** A bean bag where a lounger would be, its sitter's feet at `at`. */
export function BeanBag({
  at,
  seed,
}: {
  at: { x: number; z: number; facing: number };
  seed: number;
}) {
  const color = BEAN_COLORS[seed % BEAN_COLORS.length];
  return (
    <group position={[at.x, 0, at.z]} rotation-y={at.facing}>
      <group position-z={-0.25}>
        <mesh position-y={0.13} scale={[1, 0.45, 1.25]}>
          <sphereGeometry args={[0.34, 12, 8]} />
          <Mat color={color} />
        </mesh>
        <mesh position={[0, 0.3, -0.24]} scale={[1, 0.8, 0.55]}>
          <sphereGeometry args={[0.28, 12, 8]} />
          <Mat color={color} />
        </mesh>
      </group>
    </group>
  );
}

/** A side table with a mug on it, steaming. */
export function MugTable({
  at,
  seed,
}: {
  at: { x: number; z: number };
  seed: number;
}) {
  return (
    <group position={[at.x, 0, at.z]}>
      <mesh position-y={0.11}>
        <cylinderGeometry args={[0.025, 0.04, 0.22, 6]} />
        <Mat color={DARK} />
      </mesh>
      <mesh position-y={0.23}>
        <cylinderGeometry args={[0.13, 0.13, 0.025, 12]} />
        <Mat color={DESK} />
      </mesh>
      <group position-y={0.245}>
        <Mug color={BEAN_COLORS[seed % BEAN_COLORS.length]} />
      </group>
    </group>
  );
}

/** A coffee mug, for whatever a drink is elsewhere. */
export function Mug({ color }: { color: string }) {
  return (
    <group>
      <mesh position-y={0.05}>
        <cylinderGeometry args={[0.04, 0.04, 0.1, 10]} />
        <Mat color={color} />
      </mesh>
      <mesh position-y={0.098} rotation-x={-Math.PI / 2}>
        <circleGeometry args={[0.034, 10]} />
        <meshBasicMaterial color="#5a3a24" />
      </mesh>
      <mesh position={[0.045, 0.05, 0]}>
        <torusGeometry args={[0.022, 0.008, 4, 8]} />
        <Mat color={color} />
      </mesh>
    </group>
  );
}

/** Where tickets are sold: a reception desk, a screen on it. */
export function Reception() {
  return (
    <group>
      <mesh position-y={0.32}>
        <boxGeometry args={[0.9, 0.64, 0.4]} />
        <Mat color={DESK} />
      </mesh>
      <mesh position={[0, 0.36, 0.201]}>
        <planeGeometry args={[0.9, 0.12]} />
        <meshBasicMaterial color="#3d7bd9" />
      </mesh>
      <mesh position={[0, 0.66, 0]}>
        <boxGeometry args={[0.96, 0.04, 0.46]} />
        <Mat color="#c9a77c" />
      </mesh>
      <group position={[0.18, 0.68, -0.08]}>
        <mesh position-y={0.04}>
          <boxGeometry args={[0.03, 0.08, 0.03]} />
          <Mat color={DARK} />
        </mesh>
        <mesh position-y={0.15}>
          <boxGeometry args={[0.28, 0.17, 0.02]} />
          <Mat color={DARK} />
        </mesh>
      </group>
      <mesh position={[-0.25, 0.72, 0.05]}>
        <sphereGeometry args={[0.035, 8, 6]} />
        <Mat color="#ffd23f" />
      </mesh>
    </group>
  );
}

/** The boat, as a big rubber duck: head to the bow at +z, a rider amidships. */
export function RubberDuck() {
  const yellow = "#ffd23f";
  return (
    <group>
      <mesh position-y={0.07} scale={[0.9, 0.45, 1.5]}>
        <sphereGeometry args={[0.3, 14, 10]} />
        <Mat color={yellow} />
      </mesh>
      {/* The tail, tipped up at the stern. */}
      <mesh position={[0, 0.18, -0.4]} rotation-x={-0.9}>
        <coneGeometry args={[0.1, 0.2, 6]} />
        <Mat color={yellow} />
      </mesh>
      <group position={[0, 0.34, 0.36]}>
        <mesh>
          <sphereGeometry args={[0.16, 12, 10]} />
          <Mat color={yellow} />
        </mesh>
        <mesh position={[0, -0.03, 0.16]} scale={[1.2, 0.5, 1]}>
          <sphereGeometry args={[0.07, 8, 6]} />
          <Mat color="#ff8a2a" />
        </mesh>
        {[-1, 1].map((s) => (
          <mesh key={s} position={[s * 0.08, 0.05, 0.13]}>
            <sphereGeometry args={[0.025, 6, 5]} />
            <meshBasicMaterial color="#1d1d1f" />
          </mesh>
        ))}
      </group>
    </group>
  );
}

/** Koi, circling slowly in the pond. */
export function Koi({
  cx,
  cz,
  rx,
  rz,
}: {
  cx: number;
  cz: number;
  rx: number;
  rz: number;
}) {
  const fish = useRef<(THREE.Group | null)[]>([]);
  useFrame((state) => {
    const t = state.clock.elapsedTime;
    fish.current.forEach((f, i) => {
      if (!f) return;
      const dir = i % 2 ? 1 : -1;
      const a = dir * t * (0.25 + i * 0.05) + i * 2.1;
      const r = 0.35 + i * 0.12;
      f.position.set(
        cx + Math.cos(a) * rx * r,
        0.02,
        cz + Math.sin(a) * rz * r,
      );
      f.rotation.y = -a - (dir * Math.PI) / 2 + Math.PI / 2;
    });
  });
  return (
    <>
      {[0, 1, 2].map((i) => (
        <group
          key={i}
          ref={(g) => {
            fish.current[i] = g;
          }}
        >
          <mesh scale={[0.5, 0.3, 1]}>
            <sphereGeometry args={[0.1, 8, 6]} />
            <meshBasicMaterial color={i === 1 ? "#ffffff" : "#ff7a2a"} />
          </mesh>
          <mesh position-z={-0.11} rotation-x={-Math.PI / 2}>
            <coneGeometry args={[0.05, 0.06, 3]} />
            <meshBasicMaterial color={i === 1 ? "#ff7a2a" : "#ffffff"} />
          </mesh>
        </group>
      ))}
    </>
  );
}

/** A swivel chair, facing the viewer. */
export function DeskChair({ color = DARK }: { color?: string }) {
  return (
    <group>
      {Array.from({ length: 5 }, (_, i) => (
        <mesh key={i} position-y={0.03} rotation-y={(i / 5) * Math.PI * 2}>
          <boxGeometry args={[0.03, 0.025, 0.36]} />
          <Mat color={DARK} />
        </mesh>
      ))}
      <mesh position-y={0.11}>
        <cylinderGeometry args={[0.02, 0.02, 0.16, 5]} />
        <Mat color={STEEL} />
      </mesh>
      <mesh position-y={0.21}>
        <boxGeometry args={[0.4, 0.06, 0.38]} />
        <Mat color={color} />
      </mesh>
      <mesh position={[0, 0.48, -0.18]} rotation-x={-0.08}>
        <boxGeometry args={[0.38, 0.42, 0.05]} />
        <Mat color={color} />
      </mesh>
    </group>
  );
}

/** The supply cupboard where the garden's harvest goes. */
export function SupplyCupboard() {
  return (
    <group>
      <mesh position-y={0.45}>
        <boxGeometry args={[0.8, 0.9, 0.5]} />
        <Mat color={STEEL} />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.05, 0.48, 0.255]}>
          <boxGeometry args={[0.02, 0.12, 0.02]} />
          <Mat color={DARK} />
        </mesh>
      ))}
      <mesh position={[0, 0.45, 0.251]}>
        <planeGeometry args={[0.005, 0.86]} />
        <meshBasicMaterial color={DARK} />
      </mesh>
    </group>
  );
}

/** The pitch's backdrop: a screen with a chart going up and to the right. */
export function SlideScreen({ width }: { width: number }) {
  const w = Math.min(width - 0.4, 3.2);
  const h = w * 0.42;
  const bars = [0.3, 0.45, 0.4, 0.62, 0.8];
  return (
    <group>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[(s * w) / 2, 0.75, 0]}>
          <cylinderGeometry args={[0.03, 0.03, 1.5, 5]} />
          <Mat color={DARK} />
        </mesh>
      ))}
      <mesh position-y={1.5 - h / 2}>
        <planeGeometry args={[w, h]} />
        <meshBasicMaterial color="#fbfbfd" />
      </mesh>
      {bars.map((b, i) => (
        <mesh
          key={i}
          position={[
            -w * 0.3 + (i * w * 0.6) / (bars.length - 1),
            1.5 - h * 0.9 + (b * h * 0.7) / 2,
            0.005,
          ]}
        >
          <planeGeometry args={[w * 0.08, b * h * 0.7]} />
          <meshBasicMaterial
            color={i === bars.length - 1 ? "#2e9e5b" : "#3d7bd9"}
          />
        </mesh>
      ))}
    </group>
  );
}

/** A tie, knotted at the neck, down the front of the torso. */
export function Tie({ color }: { color: string }) {
  return (
    <group>
      {/* The shirt's collar points either side of the knot. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.05, 0.585, -0.01]}
          rotation-z={s * 0.9}
          scale={[1, 1, 0.3]}
        >
          <coneGeometry args={[0.04, 0.09, 3]} />
          <Mat color="#ffffff" />
        </mesh>
      ))}
      <mesh position-y={0.575}>
        <boxGeometry args={[0.055, 0.05, 0.04]} />
        <Mat color={color} />
      </mesh>
      <mesh position-y={0.46} rotation-y={Math.PI / 4} scale={[1, 1, 1]}>
        <cylinderGeometry args={[0.025, 0.05, 0.18, 4]} />
        <Mat color={color} />
      </mesh>
      <mesh position-y={0.35} rotation={[Math.PI, Math.PI / 4, 0]}>
        <coneGeometry args={[0.05, 0.05, 4]} />
        <Mat color={color} />
      </mesh>
    </group>
  );
}
