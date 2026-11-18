import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { Mat } from "../animals/rig";
import type { Palette } from "../palette";
import type { Stage } from "../stage";

const METAL = "#d3d7df";
const DARK_METAL = "#7d838f";
const STRIPE = "#ff8a3d";
const GLASS = "#9fe6ff";

/** Along the back: rock spires, and now and then a dome or a dish. */
export function MoonBackdrop({
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
  const pick = Math.floor(Math.abs(x) * 7.3) % 3;
  if (c === 2 && pick === 0) return <Dome x={x} z={z} s={s} />;
  if (c === 2 && pick === 1) return <Dish x={x} z={z} s={s} />;
  return (
    <group position={[x, 0, z]} scale={s} rotation-y={x * 3.1}>
      <mesh position-y={0.42} scale={[1, 1, 0.8]}>
        <coneGeometry args={[0.36, 0.85, 5]} />
        <Mat color={palette.rock} />
      </mesh>
      <mesh position={[0.26, 0.22, 0.08]} rotation-z={-0.2}>
        <coneGeometry args={[0.2, 0.45, 5]} />
        <Mat color={palette.trunk} />
      </mesh>
    </group>
  );
}

function Dome({ x, z, s }: { x: number; z: number; s: number }) {
  return (
    <group position={[x, 0, z]} scale={s}>
      <mesh>
        <sphereGeometry args={[0.48, 14, 8, 0, Math.PI * 2, 0, Math.PI / 2]} />
        <Mat color={METAL} />
      </mesh>
      <mesh rotation-x={Math.PI / 2} position-y={0.04}>
        <torusGeometry args={[0.48, 0.04, 4, 20]} />
        <Mat color={STRIPE} />
      </mesh>
      <mesh position={[0, 0.2, 0.42]} rotation-x={-0.4}>
        <circleGeometry args={[0.1, 12]} />
        <meshBasicMaterial color={GLASS} toneMapped={false} />
      </mesh>
    </group>
  );
}

function Dish({ x, z, s }: { x: number; z: number; s: number }) {
  return (
    <group position={[x, 0, z]} scale={s}>
      <mesh position-y={0.3}>
        <cylinderGeometry args={[0.04, 0.08, 0.6, 6]} />
        <Mat color={DARK_METAL} />
      </mesh>
      <group position-y={0.68} rotation-x={0.7}>
        <mesh>
          <sphereGeometry
            args={[0.36, 14, 6, 0, Math.PI * 2, Math.PI * 0.6, Math.PI * 0.4]}
          />
          <meshStandardMaterial
            color={METAL}
            flatShading
            side={THREE.DoubleSide}
          />
        </mesh>
        <mesh position-y={-0.12}>
          <cylinderGeometry args={[0.012, 0.012, 0.36, 4]} />
          <Mat color={DARK_METAL} />
        </mesh>
        <mesh position-y={0.06}>
          <sphereGeometry args={[0.03, 6, 4]} />
          <meshBasicMaterial color="#ff5c5c" />
        </mesh>
      </group>
    </group>
  );
}

/** A crater: a darker floor and a raised rim. */
export function Crater({
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
  const size = r * 0.45;
  return (
    <group position={[x, 0, z]}>
      <mesh rotation-x={-Math.PI / 2} position-y={0.003}>
        <circleGeometry args={[size, 16]} />
        <meshStandardMaterial color={palette.patch} />
      </mesh>
      <mesh rotation-x={-Math.PI / 2} position-y={0.004} scale={[1, 1, 0.3]}>
        <torusGeometry args={[size, size * 0.12, 4, 20]} />
        <Mat color={palette.ground} />
      </mesh>
    </group>
  );
}

/** A little glowing crystal, poking out of the dust. */
export function Crystal({
  x,
  z,
  c,
  palette,
}: {
  x: number;
  z: number;
  c: number;
  palette: Palette;
}) {
  const color = palette.flowers[c % palette.flowers.length];
  return (
    <mesh
      position={[x, 0.05, z]}
      scale={[1, 1.9, 1]}
      rotation-y={x * 5}
      rotation-z={(c - 1.5) * 0.15}
    >
      <octahedronGeometry args={[0.04]} />
      <meshStandardMaterial
        color={color}
        emissive={color}
        emissiveIntensity={0.6}
        flatShading
      />
    </mesh>
  );
}

/** The portal's frame: a metal ring on pylons, lit round its inside. */
export function MoonGate({ color }: { color: string }) {
  return (
    <>
      {[-1, 1].map((s) => (
        <group key={s} position={[s * 0.52, 0, 0]}>
          <mesh position-y={0.2}>
            <boxGeometry args={[0.2, 0.4, 0.24]} />
            <Mat color={METAL} />
          </mesh>
          <mesh position={[0, 0.3, 0.121]}>
            <planeGeometry args={[0.2, 0.05]} />
            <meshBasicMaterial color={STRIPE} />
          </mesh>
        </group>
      ))}
      <mesh position-y={0.68}>
        <torusGeometry args={[0.58, 0.08, 6, 24]} />
        <Mat color={METAL} />
      </mesh>
      <mesh position-y={0.68}>
        <torusGeometry args={[0.5, 0.025, 4, 24]} />
        <meshBasicMaterial color={color} toneMapped={false} />
      </mesh>
    </>
  );
}

/** Instead of a parasol: a solar panel turned to the sun. */
export function SolarPanel({ at }: { at: { x: number; z: number } }) {
  return (
    <group position={[at.x, 0, at.z]}>
      <mesh position-y={0.45}>
        <cylinderGeometry args={[0.025, 0.03, 0.9, 5]} />
        <Mat color={DARK_METAL} />
      </mesh>
      <group position-y={0.95} rotation-x={-0.6}>
        <mesh>
          <boxGeometry args={[0.9, 0.03, 0.6]} />
          <Mat color={METAL} />
        </mesh>
        {[-0.3, 0, 0.3].flatMap((px) =>
          [-0.14, 0.14].map((pz) => (
            <mesh
              key={`${px}${pz}`}
              position={[px, 0.017, pz]}
              rotation-x={-Math.PI / 2}
            >
              <planeGeometry args={[0.27, 0.25]} />
              <meshStandardMaterial
                color="#2b4ea8"
                roughness={0.3}
                metalness={0.4}
              />
            </mesh>
          )),
        )}
      </group>
    </group>
  );
}

/** A screen of light on two posts, written on in light. */
export function HoloBoard() {
  return (
    <>
      {[-0.4, 0.4].map((x) => (
        <mesh key={x} position={[x, 0.42, 0]}>
          <cylinderGeometry args={[0.02, 0.03, 0.84, 5]} />
          <Mat color={DARK_METAL} />
        </mesh>
      ))}
      <mesh position-y={0.64}>
        <planeGeometry args={[0.8, 0.5]} />
        <meshBasicMaterial
          color="#5fe1ff"
          transparent
          opacity={0.35}
          side={THREE.DoubleSide}
          depthWrite={false}
        />
      </mesh>
      {[0.14, 0.04, -0.06, -0.16].map((y, j) => (
        <mesh key={y} position={[-0.08 + j * 0.04, 0.64 + y, 0.005]}>
          <planeGeometry args={[0.5 - j * 0.1, 0.022]} />
          <meshBasicMaterial color="#d6f8ff" toneMapped={false} />
        </mesh>
      ))}
    </>
  );
}

/** The garden's shed: a small dome with a hatch. */
export function StoreDome() {
  return (
    <group>
      <mesh>
        <sphereGeometry args={[0.46, 14, 8, 0, Math.PI * 2, 0, Math.PI / 2]} />
        <Mat color={METAL} />
      </mesh>
      <mesh position={[0, 0.17, 0.44]} rotation-x={-0.15}>
        <planeGeometry args={[0.26, 0.32]} />
        <meshStandardMaterial color={DARK_METAL} />
      </mesh>
      <mesh position={[0, 0.47, 0]}>
        <cylinderGeometry args={[0.01, 0.01, 0.24, 4]} />
        <Mat color={DARK_METAL} />
      </mesh>
      <mesh position={[0, 0.6, 0]}>
        <sphereGeometry args={[0.03, 6, 4]} />
        <meshBasicMaterial color="#ff5c5c" />
      </mesh>
    </group>
  );
}

/** Where tickets are sold: a module with a window and a dish on its roof. */
export function LaunchKiosk() {
  return (
    <group>
      <mesh position-y={0.4}>
        <boxGeometry args={[0.7, 0.8, 0.55]} />
        <Mat color={METAL} />
      </mesh>
      <mesh position={[0, 0.8, 0]} rotation-z={Math.PI / 2}>
        <cylinderGeometry args={[0.28, 0.28, 0.7, 12, 1, false, 0, Math.PI]} />
        <Mat color={METAL} />
      </mesh>
      <mesh position={[0, 0.52, 0.276]}>
        <planeGeometry args={[0.44, 0.26]} />
        <meshBasicMaterial color="#1f2a3a" />
      </mesh>
      <mesh position={[0, 0.2, 0.276]}>
        <planeGeometry args={[0.7, 0.06]} />
        <meshBasicMaterial color={STRIPE} />
      </mesh>
      <group position={[0.2, 1.12, 0]} rotation-x={0.6} scale={0.45}>
        <mesh>
          <sphereGeometry
            args={[0.36, 12, 5, 0, Math.PI * 2, Math.PI * 0.6, Math.PI * 0.4]}
          />
          <meshStandardMaterial
            color={METAL}
            flatShading
            side={THREE.DoubleSide}
          />
        </mesh>
      </group>
    </group>
  );
}

/** The boat, as a little hover shuttle: bow towards +z, its pilot amidships. */
export function Shuttle() {
  const flame = useRef<THREE.Mesh>(null);
  useFrame((state) => {
    const f = flame.current;
    if (f) f.scale.z = 0.8 + Math.sin(state.clock.elapsedTime * 30) * 0.2;
  });
  return (
    <group>
      <mesh position-y={0.09} scale={[1, 1, 2.1]}>
        <cylinderGeometry args={[0.24, 0.18, 0.18, 10]} />
        <Mat color={METAL} />
      </mesh>
      <mesh position-y={0.12} scale={[1, 1, 2.1]}>
        <torusGeometry args={[0.24, 0.025, 4, 14]} />
        <Mat color={STRIPE} />
      </mesh>
      {/* The windscreen ahead of the pilot. */}
      <mesh position={[0, 0.26, 0.24]} rotation-x={-0.5}>
        <boxGeometry args={[0.3, 0.16, 0.02]} />
        <meshStandardMaterial
          color={GLASS}
          transparent
          opacity={0.6}
          depthWrite={false}
        />
      </mesh>
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.24, 0.16, -0.38]}
          rotation-z={s * -0.5}
          scale={[0.04, 1, 1]}
        >
          <coneGeometry args={[0.2, 0.3, 3]} />
          <Mat color={STRIPE} />
        </mesh>
      ))}
      <mesh
        ref={flame}
        position={[0, 0.09, -0.52]}
        rotation-x={-Math.PI / 2}
        scale={[1, 1, 1]}
      >
        <coneGeometry args={[0.09, 0.22, 8]} />
        <meshBasicMaterial color="#8fe9ff" transparent opacity={0.8} />
      </mesh>
    </group>
  );
}

/** A bubble helmet over the head, its collar at the neck. */
export function Helmet() {
  return (
    <group>
      <mesh position-y={0.04}>
        <sphereGeometry args={[0.42, 18, 12]} />
        <meshStandardMaterial
          color="#dff6ff"
          transparent
          opacity={0.16}
          roughness={0.1}
          depthWrite={false}
        />
      </mesh>
      {/* A glint on the glass. */}
      <mesh position={[-0.2, 0.34, 0.2]} rotation={[-0.9, -0.5, 0.5]}>
        <planeGeometry args={[0.14, 0.05]} />
        <meshBasicMaterial color="#ffffff" transparent opacity={0.7} />
      </mesh>
      <mesh position-y={-0.3} rotation-x={Math.PI / 2}>
        <torusGeometry args={[0.27, 0.05, 6, 18]} />
        <Mat color={METAL} />
      </mesh>
    </group>
  );
}

/** An air tank on its back, from the torso's centre. */
export function AirTank() {
  return (
    <group position={[0, 0.42, -0.24]}>
      {[-0.07, 0.07].map((x) => (
        <mesh key={x} position-x={x}>
          <capsuleGeometry args={[0.065, 0.18, 3, 8]} />
          <Mat color={METAL} />
        </mesh>
      ))}
      <mesh position-y={0.15}>
        <boxGeometry args={[0.2, 0.03, 0.06]} />
        <Mat color={STRIPE} />
      </mesh>
    </group>
  );
}

/** A meteor every few seconds, streaking across the top of either meadow. */
export function Meteors({ stage }: { stage: Stage }) {
  const COUNT = 3;
  const streaks = useRef<(THREE.Mesh | null)[]>([]);
  const runs = useMemo(
    () =>
      Array.from({ length: COUNT }, (_, i) => ({ next: 2 + i * 4, seed: i })),
    [],
  );
  const live = useRef(
    Array.from({ length: COUNT }, () => ({
      from: new THREE.Vector3(),
      dir: new THREE.Vector3(),
      at: -1,
    })),
  );
  useFrame((state) => {
    const t = state.clock.elapsedTime;
    const v = stage.visible;
    runs.forEach((run, i) => {
      const m = streaks.current[i];
      const l = live.current[i];
      if (!m) return;
      if (t >= run.next) {
        // A side and a start at random, heading down and across.
        const left = Math.random() < 0.5;
        const xMin = left ? v.xMin : stage.panel.xMax;
        const xMax = left ? stage.panel.xMin : v.xMax;
        l.from.set(
          xMin + Math.random() * (xMax - xMin),
          2 + Math.random() * 1.5,
          v.zMin + (0.3 + Math.random() * 0.4) * (v.zMax - v.zMin),
        );
        const side = Math.random() < 0.5 ? -1 : 1;
        l.dir.set(side * 6, -2.5, 3).normalize();
        l.at = t;
        run.next = t + 5 + Math.random() * 9;
      }
      const age = t - l.at;
      const life = 0.7;
      m.visible = l.at >= 0 && age < life;
      if (!m.visible) return;
      m.position.copy(l.from).addScaledVector(l.dir, age * 9);
      m.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), l.dir);
      (m.material as THREE.MeshBasicMaterial).opacity = 1 - age / life;
    });
  });
  return (
    <>
      {runs.map((_, i) => (
        <mesh
          key={i}
          ref={(m) => {
            streaks.current[i] = m;
          }}
          visible={false}
        >
          <cylinderGeometry args={[0.05, 0.002, 2.2, 5]} />
          <meshBasicMaterial
            color="#fff6dc"
            transparent
            blending={THREE.AdditiveBlending}
            depthWrite={false}
          />
        </mesh>
      ))}
    </>
  );
}
