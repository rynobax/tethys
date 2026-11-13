import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import type {
  Lake,
  Lounge,
  Pitch,
  Region,
  Seat,
  Side,
  Sim,
  Vec2,
  Workshop,
} from "./sim";
import { STAGE_HEIGHT, type Stage } from "./stage";

/** Everything about the look that isn't a resident. Swap it to reskin. */
export interface Environment {
  ground: string;
  patch: string;
  sky: string;
  hemiGround: string;
  hemiIntensity: number;
  sun: string;
  sunIntensity: number;
  foliage: string[];
  trunk: string;
  rock: string;
  flowers: string[];
  portal: string;
  /** The water in the lounge's pool. */
  water: string;
  /** How much the pool lights itself, for an evening swim. */
  waterGlow: number;
  fireflies: boolean;
}

export const MEADOW: Environment = {
  ground: "#a9d673",
  patch: "#9ccb64",
  sky: "#eef8ff",
  hemiGround: "#8fbf5a",
  hemiIntensity: 1.6,
  sun: "#fff3dc",
  sunIntensity: 1.9,
  foliage: ["#5fb35a", "#4fa152", "#77c26a"],
  trunk: "#9a6b47",
  rock: "#bdb8b0",
  flowers: ["#ff8fab", "#ffd166", "#ffffff", "#b39ddb"],
  portal: "#9b6bff",
  water: "#4fc3e8",
  waterGlow: 0.05,
  fireflies: false,
};

export const DUSK: Environment = {
  ground: "#3e6a4e",
  patch: "#376146",
  sky: "#9fb2e0",
  hemiGround: "#2c4a3a",
  hemiIntensity: 1.6,
  sun: "#ffc98f",
  sunIntensity: 1.6,
  foliage: ["#2f6b4a", "#285e41", "#3a7a55"],
  trunk: "#5d4636",
  rock: "#7c7f86",
  flowers: ["#f48fb1", "#ffe082", "#b39ddb"],
  portal: "#b388ff",
  water: "#2a9fd0",
  waterGlow: 0.45,
  fireflies: true,
};

/** Deterministic, so a resize to the same size draws the same meadow. */
function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface Decor {
  trees: { x: number; z: number; s: number; c: number }[];
  rocks: { x: number; z: number; s: number; r: number }[];
  flowers: { x: number; z: number; c: number }[];
  tufts: { x: number; z: number }[];
  patches: { x: number; z: number; r: number }[];
}

function scatter(stage: Stage, side: Side): Decor {
  const region = stage.layout.regions[side];
  const decor: Decor = {
    trees: [],
    rocks: [],
    flowers: [],
    tufts: [],
    patches: [],
  };
  if (!region) return decor;
  const v = stage.visible;
  const xMin = side === "left" ? v.xMin : stage.panel.xMax;
  const xMax = side === "left" ? stage.panel.xMin : v.xMax;
  const rng = mulberry32(
    Math.round((xMax - xMin) * 10) * 7919 +
      Math.round((v.zMax - v.zMin) * 10) +
      (side === "left" ? 1 : 2),
  );
  const portal = stage.layout.portals[side];

  // Nothing grows in the zones, where it'd be underfoot.
  const { lounge, workshop, pitch, lake } = stage.layout;
  const zones = [
    lounge?.area,
    workshop?.area,
    lake && {
      ...lake.water,
      xMax: lake.water.xMax + 0.4,
      zMax: lake.water.zMax + 0.4,
    },
    // The stage and the crowd in front of it.
    pitch && { ...pitch.stage, zMax: v.zMax },
  ];
  const inZone = (p: Vec2) =>
    zones.some(
      (z) => z && p.x > z.xMin && p.x < z.xMax && p.z > z.zMin && p.z < z.zMax,
    );
  // A treeline along the back, leaving the portal and the lake clear.
  for (let x = xMin + 0.3; x < xMax; x += 0.7 + rng() * 0.6) {
    const tree = {
      x,
      z: v.zMin + 0.4 + rng() * (region.zMin - v.zMin - 0.6),
      s: 0.8 + rng() * 0.6,
      c: Math.floor(rng() * 3),
    };
    if (portal && Math.abs(x - portal.x) < 1.1) continue;
    if (!inZone(tree)) decor.trees.push(tree);
  }
  // And a few down the outer edge.
  const outer = side === "left" ? xMin + 0.35 : xMax - 0.35;
  for (let z = region.zMin + 1; z < v.zMax; z += 1.6 + rng() * 1.8) {
    const tree = {
      x: outer + (rng() - 0.5) * 0.3,
      z,
      s: 0.6 + rng() * 0.5,
      c: Math.floor(rng() * 3),
    };
    if (!inZone(tree)) decor.trees.push(tree);
  }
  const area = (region.xMax - region.xMin) * (region.zMax - region.zMin);
  const point = () => ({
    x: region.xMin + rng() * (region.xMax - region.xMin),
    z: region.zMin + rng() * (region.zMax - region.zMin),
  });
  for (let i = 0; i < area * 0.04; i++) {
    const p = point();
    if (inZone(p)) continue;
    decor.rocks.push({ ...p, s: 0.12 + rng() * 0.16, r: rng() * Math.PI });
  }
  for (let i = 0; i < area * 0.35; i++) {
    const p = point();
    const c = Math.floor(rng() * 4);
    if (!inZone(p)) decor.flowers.push({ ...p, c });
  }
  for (let i = 0; i < area * 0.3; i++) {
    const p = point();
    if (!inZone(p)) decor.tufts.push(p);
  }
  for (let i = 0; i < area * 0.03 + 1; i++)
    decor.patches.push({ ...point(), r: 0.8 + rng() * 1.6 });
  return decor;
}

function Tree({
  x,
  z,
  s,
  color,
  trunk,
}: {
  x: number;
  z: number;
  s: number;
  color: string;
  trunk: string;
}) {
  return (
    <group position={[x, 0, z]} scale={s}>
      <mesh position-y={0.2}>
        <cylinderGeometry args={[0.07, 0.09, 0.4, 6]} />
        <meshStandardMaterial color={trunk} flatShading />
      </mesh>
      <mesh position-y={0.62}>
        <coneGeometry args={[0.42, 0.7, 7]} />
        <meshStandardMaterial color={color} flatShading />
      </mesh>
      <mesh position-y={0.95}>
        <coneGeometry args={[0.32, 0.55, 7]} />
        <meshStandardMaterial color={color} flatShading />
      </mesh>
    </group>
  );
}

/** A ring of stone and light, turned `turn` radians about y. */
function Portal({
  at,
  color,
  turn,
}: {
  at: Vec2;
  color: string;
  turn: number;
}) {
  const swirl = useRef<THREE.Mesh>(null);
  const sparks = useRef<THREE.Group>(null);
  useFrame((state) => {
    const t = state.clock.elapsedTime;
    if (swirl.current) {
      swirl.current.rotation.z = t * 1.5;
      (swirl.current.material as THREE.MeshBasicMaterial).opacity =
        0.55 + Math.sin(t * 2) * 0.15;
    }
    if (sparks.current) sparks.current.rotation.z = -t * 0.9;
  });
  return (
    <group position={[at.x, 0, at.z - 0.15]} rotation-y={turn}>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * 0.5, 0.08, 0]}>
          <boxGeometry args={[0.26, 0.16, 0.22]} />
          <meshStandardMaterial color="#8d8a85" flatShading />
        </mesh>
      ))}
      <mesh position-y={0.68}>
        <torusGeometry args={[0.55, 0.09, 8, 24]} />
        <meshStandardMaterial
          color={color}
          emissive={color}
          emissiveIntensity={0.45}
          flatShading
        />
      </mesh>
      <mesh ref={swirl} position-y={0.68}>
        <circleGeometry args={[0.5, 6]} />
        <meshBasicMaterial
          color={color}
          transparent
          opacity={0.6}
          side={THREE.DoubleSide}
        />
      </mesh>
      <group ref={sparks} position-y={0.68}>
        {[0, 1, 2, 3, 4].map((i) => (
          <mesh
            key={i}
            position={[
              Math.cos((i / 5) * Math.PI * 2) * 0.72,
              Math.sin((i / 5) * Math.PI * 2) * 0.72,
              0,
            ]}
          >
            <octahedronGeometry args={[0.04]} />
            <meshBasicMaterial color="#ffffff" />
          </mesh>
        ))}
      </group>
    </group>
  );
}

function Fireflies({ stage }: { stage: Stage }) {
  const count = 40;
  const points = useRef<THREE.Points>(null);
  const seeds = useMemo(() => {
    const rng = mulberry32(42);
    const v = stage.visible;
    return Array.from({ length: count }, () => {
      const left = rng() < 0.5;
      const xMin = left ? v.xMin : stage.panel.xMax;
      const xMax = left ? stage.panel.xMin : v.xMax;
      return {
        x: xMin + rng() * (xMax - xMin),
        z: v.zMin + rng() * (v.zMax - v.zMin),
        y: 0.3 + rng() * 1.4,
        p: rng() * Math.PI * 2,
      };
    });
  }, [stage]);
  const positions = useMemo(() => new Float32Array(count * 3), []);
  // A soft dot rather than the square a bare point draws as.
  const glow = useMemo(() => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 32;
    const ctx = canvas.getContext("2d")!;
    const g = ctx.createRadialGradient(16, 16, 0, 16, 16, 16);
    g.addColorStop(0, "rgba(255,255,255,1)");
    g.addColorStop(0.35, "rgba(255,255,255,0.6)");
    g.addColorStop(1, "rgba(255,255,255,0)");
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, 32, 32);
    return new THREE.CanvasTexture(canvas);
  }, []);
  useFrame((state) => {
    const t = state.clock.elapsedTime;
    seeds.forEach((s, i) => {
      positions[i * 3] = s.x + Math.sin(t * 0.4 + s.p) * 0.6;
      positions[i * 3 + 1] = s.y + Math.sin(t * 0.9 + s.p * 2) * 0.25;
      positions[i * 3 + 2] = s.z + Math.cos(t * 0.3 + s.p) * 0.6;
    });
    const geo = points.current?.geometry;
    if (geo) geo.attributes.position.needsUpdate = true;
  });
  return (
    <points ref={points}>
      <bufferGeometry>
        <bufferAttribute attach="attributes-position" args={[positions, 3]} />
      </bufferGeometry>
      <pointsMaterial
        map={glow}
        color="#f6ff9e"
        size={9}
        sizeAttenuation={false}
        transparent
        opacity={0.85}
        blending={THREE.AdditiveBlending}
        depthWrite={false}
      />
    </points>
  );
}

const ONLOOKER_COLOURS = [
  "#c9b8e8",
  "#f2c4a0",
  "#a8d5c2",
  "#f0b8c8",
  "#b8cdeb",
  "#e8dca0",
];

/**
 * One of the crowd a Shipping resident pitches to: small, faceless from
 * behind, and given to clapping or putting a hand up with a question.
 */
function Onlooker({ at, seed }: { at: Vec2; seed: number }) {
  const body = useRef<THREE.Group>(null);
  const armL = useRef<THREE.Group>(null);
  const armR = useRef<THREE.Group>(null);
  const colour = ONLOOKER_COLOURS[seed % ONLOOKER_COLOURS.length];
  const phase = (seed * 2.399) % (Math.PI * 2);
  useFrame((state) => {
    const t = state.clock.elapsedTime + phase;
    const b = body.current;
    const L = armL.current;
    const R = armR.current;
    if (!b || !L || !R) return;
    // Each takes turns at a few seconds of clapping, asking, or just watching.
    const mode = Math.floor(t / 4 + seed) % 5;
    b.position.y = 0;
    L.rotation.set(-0.2, 0, -0.15);
    R.rotation.set(-0.2, 0, 0.15);
    if (mode === 0) {
      L.rotation.set(-1.2, 0, 0.35 + Math.sin(t * 14) * 0.25);
      R.rotation.set(-1.2, 0, -0.35 - Math.sin(t * 14) * 0.25);
      b.position.y = Math.abs(Math.sin(t * 7)) * 0.04;
    } else if (mode === 1) {
      R.rotation.z = 2.7 + Math.sin(t * 6) * 0.15;
    } else {
      b.position.y = Math.sin(t * 1.8) * 0.015;
    }
  });
  return (
    <group position={[at.x, 0, at.z]} rotation-y={Math.PI} scale={0.55}>
      <mesh rotation-x={-Math.PI / 2} position-y={0.01}>
        <circleGeometry args={[0.34, 14]} />
        <meshBasicMaterial
          color="#000000"
          transparent
          opacity={0.15}
          depthWrite={false}
        />
      </mesh>
      <group ref={body}>
        <mesh position-y={0.58}>
          <capsuleGeometry args={[0.3, 0.36, 4, 10]} />
          <meshStandardMaterial color={colour} flatShading roughness={0.8} />
        </mesh>
        <group ref={armL} position={[-0.32, 0.56, 0]}>
          <mesh position-y={-0.14}>
            <capsuleGeometry args={[0.06, 0.16, 3, 6]} />
            <meshStandardMaterial color={colour} flatShading />
          </mesh>
        </group>
        <group ref={armR} position={[0.32, 0.56, 0]}>
          <mesh position-y={-0.14}>
            <capsuleGeometry args={[0.06, 0.16, 3, 6]} />
            <meshStandardMaterial color={colour} flatShading />
          </mesh>
        </group>
      </group>
    </group>
  );
}

/** A low stage with bunting at the back, and its audience in front. */
/** A stage chair, drawn only while someone has it, so the queue can be counted. */
/**
 * The row behind the podium is always set out; a row behind that is put out
 * whole, and the stage grows back to hold it, as soon as the queue needs it.
 */
function Chair({ at, row, sim }: { at: Vec2; row: number; sim: Sim }) {
  const group = useRef<THREE.Group>(null);
  useFrame(() => {
    if (!group.current) return;
    group.current.visible = row < sim.chairRowsOut();
  });
  const wood = "#7a5236";
  return (
    <group ref={group} position={[at.x, STAGE_HEIGHT, at.z]} visible={false}>
      <mesh position-y={0.16}>
        <boxGeometry args={[0.4, 0.04, 0.36]} />
        <meshStandardMaterial color={wood} flatShading />
      </mesh>
      {[
        [-0.17, -0.15],
        [0.17, -0.15],
        [-0.17, 0.15],
        [0.17, 0.15],
      ].map(([x, z]) => (
        <mesh key={`${x}${z}`} position={[x, 0.07, z]}>
          <boxGeometry args={[0.04, 0.14, 0.04]} />
          <meshStandardMaterial color={wood} flatShading />
        </mesh>
      ))}
      <mesh position={[0, 0.34, -0.17]}>
        <boxGeometry args={[0.4, 0.32, 0.04]} />
        <meshStandardMaterial color={wood} flatShading />
      </mesh>
    </group>
  );
}

/** Rings in the pool's colours, one per ring, so neighbours differ. */
const RING_COLORS = ["#ff6fa3", "#ffd23f", "#5ee0b0"];

/** An inflatable ring, with white stripes round it. */
export function FloatRing({ index }: { index: number }) {
  const color = RING_COLORS[index % RING_COLORS.length];
  return (
    <group>
      <mesh rotation-x={Math.PI / 2}>
        <torusGeometry args={[0.32, 0.12, 8, 18]} />
        <meshStandardMaterial color={color} flatShading />
      </mesh>
      {[0, 1, 2, 3].map((i) => {
        const a = (i / 4) * Math.PI * 2 + 0.4;
        return (
          <mesh
            key={i}
            position={[Math.cos(a) * 0.32, 0, Math.sin(a) * 0.32]}
            rotation-y={-a}
          >
            <boxGeometry args={[0.06, 0.25, 0.25]} />
            <meshStandardMaterial color="#ffffff" flatShading />
          </mesh>
        );
      })}
    </group>
  );
}

/** A ring nobody is asleep on, bobbing about on the water. */
function EmptyRing({ at, index, sim }: { at: Vec2; index: number; sim: Sim }) {
  const group = useRef<THREE.Group>(null);
  useFrame((state) => {
    const g = group.current;
    if (!g) return;
    g.visible = !sim.napTaken(index);
    const t = state.clock.elapsedTime + index * 2.1;
    g.position.set(at.x + Math.sin(t * 0.3) * 0.15, 0.03, at.z);
    g.rotation.set(
      Math.sin(t * 1.3) * 0.05,
      t * 0.15,
      Math.cos(t * 1.1) * 0.05,
    );
  });
  return (
    <group ref={group} scale={0.7}>
      <FloatRing index={index} />
    </group>
  );
}

/** A reclining sun lounger, its sitter's feet at `at` and its back behind. */
function Lounger({ at }: { at: Seat }) {
  const frame = "#ffffff";
  const cushion = "#5bb8d6";
  return (
    <group position={[at.x, 0, at.z]} rotation-y={at.facing}>
      <group position-z={-0.2}>
        <mesh position-y={0.13}>
          <boxGeometry args={[0.42, 0.05, 0.9]} />
          <meshStandardMaterial color={frame} flatShading />
        </mesh>
        <mesh position={[0, 0.17, 0.08]}>
          <boxGeometry args={[0.36, 0.04, 0.62]} />
          <meshStandardMaterial color={cushion} flatShading />
        </mesh>
        {/* The back, tilted up from the head end. */}
        <mesh position={[0, 0.33, -0.36]} rotation-x={0.9}>
          <boxGeometry args={[0.38, 0.05, 0.5]} />
          <meshStandardMaterial color={cushion} flatShading />
        </mesh>
        {[-0.17, 0.17].flatMap((x) =>
          [-0.38, 0.38].map((z) => (
            <mesh key={`${x}${z}`} position={[x, 0.055, z]}>
              <boxGeometry args={[0.04, 0.11, 0.04]} />
              <meshStandardMaterial color={frame} flatShading />
            </mesh>
          )),
        )}
      </group>
    </group>
  );
}

/** A little round table with a drink on it, umbrella and all. */
function DrinkTable({ at, seed }: { at: Vec2; seed: number }) {
  const juice = ["#ff8a5c", "#ffd23f", "#ff5c8a", "#7be07b"][seed % 4];
  return (
    <group position={[at.x, 0, at.z]}>
      <mesh position-y={0.11}>
        <cylinderGeometry args={[0.025, 0.04, 0.22, 6]} />
        <meshStandardMaterial color="#ffffff" flatShading />
      </mesh>
      <mesh position-y={0.23}>
        <cylinderGeometry args={[0.13, 0.13, 0.025, 12]} />
        <meshStandardMaterial color="#ffffff" flatShading />
      </mesh>
      <group position-y={0.245}>
        <Drink juice={juice} />
      </group>
    </group>
  );
}

/** A tall glass of something bright, with a straw and a paper umbrella. */
export function Drink({ juice }: { juice: string }) {
  return (
    <group>
      <mesh position-y={0.06}>
        <cylinderGeometry args={[0.04, 0.032, 0.12, 8]} />
        <meshStandardMaterial
          color="#e8fbff"
          transparent
          opacity={0.45}
          depthWrite={false}
        />
      </mesh>
      <mesh position-y={0.05}>
        <cylinderGeometry args={[0.034, 0.028, 0.09, 8]} />
        <meshStandardMaterial color={juice} />
      </mesh>
      <mesh position={[0.012, 0.14, 0]} rotation-z={-0.25}>
        <cylinderGeometry args={[0.006, 0.006, 0.12, 4]} />
        <meshBasicMaterial color="#ff4f7b" />
      </mesh>
      <group position={[-0.02, 0.16, 0]} rotation-z={0.4}>
        <mesh position-y={0.03}>
          <cylinderGeometry args={[0.003, 0.003, 0.07, 3]} />
          <meshBasicMaterial color="#c9a46b" />
        </mesh>
        <mesh position-y={0.07}>
          <coneGeometry args={[0.05, 0.025, 8]} />
          <meshStandardMaterial color="#ffd23f" flatShading />
        </mesh>
      </group>
    </group>
  );
}

/** A beach parasol, striped. */
function Parasol({ at, seed }: { at: Vec2; seed: number }) {
  const colors = seed % 2 ? ["#ff6f61", "#ffffff"] : ["#3fa9f5", "#ffffff"];
  return (
    <group position={[at.x, 0, at.z]}>
      <mesh position-y={0.6}>
        <cylinderGeometry args={[0.02, 0.02, 1.2, 5]} />
        <meshStandardMaterial color="#e8e2d4" flatShading />
      </mesh>
      {Array.from({ length: 8 }, (_, i) => (
        <mesh key={i} position-y={1.15} rotation-y={(i / 8) * Math.PI * 2}>
          <coneGeometry args={[0.6, 0.22, 2, 1, true, 0, (Math.PI * 2) / 8]} />
          <meshStandardMaterial
            color={colors[i % 2]}
            flatShading
            side={THREE.DoubleSide}
          />
        </mesh>
      ))}
    </group>
  );
}

/** A pool, loungers down either side, rings on the water. */
function LoungeArea({
  lounge,
  env,
  sim,
}: {
  lounge: Lounge;
  env: Environment;
  sim: Sim;
}) {
  const water = useRef<THREE.MeshStandardMaterial>(null);
  const glints = useRef<THREE.Group>(null);
  useFrame((state) => {
    const t = state.clock.elapsedTime;
    glints.current?.children.forEach((g, i) => {
      g.scale.x = 0.6 + Math.sin(t * 1.7 + i * 1.9) * 0.4;
      g.position.x = Math.sin(t * 0.4 + i) * 0.1 + ((i * 0.37) % 1) - 0.5;
    });
    if (water.current) {
      water.current.emissiveIntensity =
        env.waterGlow * (1 + Math.sin(t * 1.2) * 0.15);
    }
  });
  const { pool } = lounge;
  const w = pool.xMax - pool.xMin;
  const d = pool.zMax - pool.zMin;
  const cx = (pool.xMin + pool.xMax) / 2;
  const cz = (pool.zMin + pool.zMax) / 2;
  const glintCount = Math.max(3, Math.floor(d / 1.2));
  return (
    <>
      {/* The coping, then the water a little below it. */}
      <mesh position={[cx, 0.025, cz]}>
        <boxGeometry args={[w + 0.24, 0.05, d + 0.24]} />
        <meshStandardMaterial color="#ffffff" flatShading />
      </mesh>
      <mesh rotation-x={-Math.PI / 2} position={[cx, 0.052, cz]}>
        <planeGeometry args={[w, d]} />
        <meshStandardMaterial
          ref={water}
          color={env.water}
          emissive={env.water}
          emissiveIntensity={env.waterGlow}
        />
      </mesh>
      <group ref={glints} position={[cx, 0.056, cz]}>
        {Array.from({ length: glintCount }, (_, i) => (
          <mesh
            key={i}
            rotation-x={-Math.PI / 2}
            position-z={-d / 2 + ((i + 0.5) * d) / glintCount}
          >
            <planeGeometry args={[w * 0.35, 0.04]} />
            <meshBasicMaterial color="#ffffff" transparent opacity={0.55} />
          </mesh>
        ))}
      </group>
      {/* A ladder at the near end. */}
      {[-0.12, 0.12].map((x) => (
        <mesh
          key={x}
          position={[pool.xMax - 0.3 + x, 0.2, pool.zMax - 0.05]}
          rotation-x={-0.3}
        >
          <torusGeometry args={[0.12, 0.015, 4, 10, Math.PI]} />
          <meshStandardMaterial color="#d8dde3" />
        </mesh>
      ))}
      {lounge.naps.map((p, i) => (
        <EmptyRing key={i} at={p} index={i} sim={sim} />
      ))}
      {lounge.seats.map((p, i) => (
        <Lounger key={i} at={p} />
      ))}
      {lounge.tables.map((p, i) => (
        <DrinkTable key={i} at={p} seed={i} />
      ))}
      {lounge.parasols.map((p, i) => (
        <Parasol key={i} at={p} seed={i} />
      ))}
    </>
  );
}

/** A little sailboat, its bow towards +z; a sailor sits amidships. */
export function Boat() {
  const hull = "#c8714a";
  return (
    <group>
      <mesh position-y={0.08} scale={[1, 1, 2.2]}>
        <cylinderGeometry args={[0.26, 0.17, 0.18, 10]} />
        <meshStandardMaterial color={hull} flatShading />
      </mesh>
      <mesh position-y={0.175} scale={[1, 1, 2.2]}>
        <torusGeometry args={[0.25, 0.03, 4, 14]} />
        <meshStandardMaterial color="#f4ead8" flatShading />
      </mesh>
      <mesh position={[0, 0.65, 0.18]}>
        <cylinderGeometry args={[0.018, 0.018, 1, 5]} />
        <meshStandardMaterial color="#7a5236" flatShading />
      </mesh>
      {/* The sail, a flattened triangle off the mast. */}
      <mesh position={[0, 0.66, 0]} scale={[0.05, 1, 1]}>
        <coneGeometry args={[0.3, 0.8, 3]} />
        <meshStandardMaterial color="#ffffff" flatShading />
      </mesh>
      <mesh position={[0, 1.18, 0.18]}>
        <coneGeometry args={[0.07, 0.12, 3]} />
        <meshStandardMaterial color="#ff5c8a" flatShading />
      </mesh>
    </group>
  );
}

/**
 * An irregular, unit-sized shoreline: a few slow waves round a circle, kept
 * inside it so the water stays within the rectangle walkers keep out of.
 * `grow` widens it into the sandy rim round the water.
 */
function shorelineShape(grow: number): THREE.Shape {
  const shape = new THREE.Shape();
  const steps = 64;
  for (let i = 0; i <= steps; i++) {
    const a = (i / steps) * Math.PI * 2;
    const r =
      0.86 +
      grow +
      Math.sin(a * 2 + 0.6) * 0.06 +
      Math.sin(a * 3 + 2.1) * 0.05 +
      Math.sin(a * 5 + 0.3) * 0.025;
    // Shape y becomes world -z once laid flat.
    const x = Math.cos(a) * r;
    const y = -Math.sin(a) * r;
    if (i === 0) shape.moveTo(x, y);
    else shape.lineTo(x, y);
  }
  return shape;
}

/**
 * Water running off the window's edge with a sandy rim, lily pads, reeds, and
 * a pier out into it with a boat waiting at the end.
 */
function LakeArea({
  lake,
  env,
  sim,
}: {
  lake: Lake;
  env: Environment;
  sim: Sim;
}) {
  const boat = useRef<THREE.Group>(null);
  const water = useRef<THREE.MeshStandardMaterial>(null);
  useFrame((state) => {
    const t = state.clock.elapsedTime;
    const b = boat.current;
    if (b) {
      b.visible = !sim.sailing();
      b.position.y = Math.sin(t * 1.4) * 0.015;
      b.rotation.z = Math.sin(t * 1.1) * 0.04;
    }
    if (water.current) {
      water.current.emissiveIntensity =
        env.waterGlow * 0.5 * (1 + Math.sin(t * 0.9) * 0.15);
    }
  });
  const { water: w, pierFoot, pierEnd } = lake;
  const cx = (w.xMin + w.xMax) / 2;
  const cz = (w.zMin + w.zMax) / 2;
  const rx = (w.xMax - w.xMin) / 2;
  const rz = (w.zMax - w.zMin) / 2;
  const [shoreline, sand] = useMemo(
    () => [shorelineShape(0), shorelineShape(0.12)],
    [],
  );
  const pierLength = pierFoot.x - pierEnd.x;
  const posts = Math.max(2, Math.round(pierLength / 0.5));
  return (
    <>
      <mesh
        rotation-x={-Math.PI / 2}
        position={[cx, 0.006, cz]}
        scale={[rx, rz, 1]}
      >
        <shapeGeometry args={[sand]} />
        <meshStandardMaterial color="#e3cf9a" />
      </mesh>
      <mesh
        rotation-x={-Math.PI / 2}
        position={[cx, 0.012, cz]}
        scale={[rx, rz, 1]}
      >
        <shapeGeometry args={[shoreline]} />
        <meshStandardMaterial
          ref={water}
          color={env.water}
          emissive={env.water}
          emissiveIntensity={env.waterGlow * 0.5}
        />
      </mesh>
      {[
        [0.55, -0.35],
        [0.7, 0.45],
        [0.35, 0.55],
      ].map(([fx, fz], i) => (
        <mesh
          key={i}
          rotation-x={-Math.PI / 2}
          position={[cx + fx * rx, 0.016, cz + fz * rz]}
        >
          <circleGeometry args={[0.14, 9, 0.4, Math.PI * 1.8]} />
          <meshStandardMaterial color="#5fae5a" side={THREE.DoubleSide} />
        </mesh>
      ))}
      {[-0.7, -0.55, -0.4, 0.75, 0.88].map((f, i) => (
        <mesh
          key={i}
          position={[cx + Math.sqrt(1 - f * f) * rx - 0.1, 0.18, cz + f * rz]}
          rotation-z={(i % 3) * 0.12 - 0.1}
        >
          <cylinderGeometry args={[0.012, 0.016, 0.36, 4]} />
          <meshStandardMaterial color="#6f9a3c" flatShading />
        </mesh>
      ))}
      {/* The pier: planks on posts, from the shore out over the water. */}
      <mesh position={[(pierFoot.x + pierEnd.x) / 2, 0.07, pierEnd.z]}>
        <boxGeometry args={[pierLength + 0.3, 0.04, 0.42]} />
        <meshStandardMaterial color="#a77b50" flatShading />
      </mesh>
      {Array.from({ length: posts }, (_, i) =>
        [-0.19, 0.19].map((dz) => (
          <mesh
            key={`${i}${dz}`}
            position={[
              pierEnd.x + (i / (posts - 1)) * pierLength,
              0.05,
              pierEnd.z + dz,
            ]}
          >
            <cylinderGeometry args={[0.03, 0.03, 0.14, 5]} />
            <meshStandardMaterial color="#7a5236" flatShading />
          </mesh>
        )),
      )}
      {/* Moored alongside the pier's end, bow out to the open water. */}
      <group position={[pierEnd.x - 0.2, 0.02, pierEnd.z + 0.45]}>
        <group ref={boat} rotation-y={-Math.PI / 2} scale={0.75}>
          <Boat />
        </group>
      </group>
    </>
  );
}

/** A paved yard, two whiteboards along the back, and two piles of crates. */
function WorkshopArea({ workshop }: { workshop: Workshop }) {
  return (
    <>
      {workshop.boards.map((p, i) => (
        <group key={i} position={[p.x, 0, p.z - 0.5]}>
          {[-0.36, 0.36].map((x) => (
            <mesh key={x} position={[x, 0.42, 0]}>
              <cylinderGeometry args={[0.025, 0.025, 0.84, 5]} />
              <meshStandardMaterial color="#6b6f76" flatShading />
            </mesh>
          ))}
          <mesh position-y={0.62}>
            <boxGeometry args={[0.8, 0.48, 0.03]} />
            <meshStandardMaterial color="#f7f7f2" />
          </mesh>
          {/* A few scribbles, so it reads as written on. */}
          {[0.12, 0.02, -0.08].map((y, j) => (
            <mesh key={y} position={[-0.06 + j * 0.05, 0.62 + y, 0.017]}>
              <planeGeometry args={[0.5 - j * 0.12, 0.025]} />
              <meshBasicMaterial color={["#3d6fd1", "#d14d3d", "#2e9e5b"][j]} />
            </mesh>
          ))}
        </group>
      ))}
      {workshop.piles.map((p, i) => (
        <group key={i} position={[p.x, 0, p.z]}>
          {[
            [0, 0, 0],
            [0.22, 0, 0.05],
            [0.1, 0.2, 0.02],
          ].map(([x, y, z], j) => (
            <mesh key={j} position={[x - 0.1, y + 0.1, z]} rotation-y={j * 0.3}>
              <boxGeometry args={[0.2, 0.2, 0.2]} />
              <meshStandardMaterial color="#c08a5b" flatShading />
            </mesh>
          ))}
        </group>
      ))}
    </>
  );
}

/** The stage itself: boards on a low platform, and bunting across the back. */
function Boards({ stage, env }: { stage: Region; env: Environment }) {
  const w = stage.xMax - stage.xMin;
  const d = stage.zMax - stage.zMin;
  const cx = (stage.xMin + stage.xMax) / 2;
  const cz = (stage.zMin + stage.zMax) / 2;
  const flags = Math.max(4, Math.round(w / 0.45));
  return (
    <>
      <mesh position={[cx, STAGE_HEIGHT / 2, cz]}>
        <boxGeometry args={[w, STAGE_HEIGHT, d]} />
        <meshStandardMaterial color="#b98a5e" flatShading />
      </mesh>
      {/* Board seams, so it reads as a stage rather than a slab. */}
      {Array.from({ length: Math.floor(d / 0.5) }, (_, i) => (
        <mesh
          key={i}
          position={[cx, STAGE_HEIGHT + 0.002, stage.zMin + 0.5 * (i + 1)]}
        >
          <boxGeometry args={[w, 0.004, 0.03]} />
          <meshBasicMaterial color="#9a7049" />
        </mesh>
      ))}
      {[stage.xMin + 0.1, stage.xMax - 0.1].map((x) => (
        <mesh key={x} position={[x, 0.75, stage.zMin + 0.1]}>
          <cylinderGeometry args={[0.04, 0.05, 1.5, 6]} />
          <meshStandardMaterial color={env.trunk} flatShading />
        </mesh>
      ))}
      {Array.from({ length: flags }, (_, i) => {
        const f = (i + 0.5) / flags;
        const sag = Math.sin(f * Math.PI) * 0.25;
        return (
          <mesh
            key={i}
            position={[
              stage.xMin + 0.1 + f * (w - 0.2),
              1.38 - sag,
              stage.zMin + 0.1,
            ]}
            rotation-z={Math.PI}
          >
            <coneGeometry args={[0.1, 0.2, 3]} />
            <meshStandardMaterial
              color={env.flowers[i % env.flowers.length]}
              flatShading
            />
          </mesh>
        );
      })}
    </>
  );
}

function PitchArea({
  pitch,
  env,
  sim,
}: {
  pitch: Pitch;
  env: Environment;
  sim: Sim;
}) {
  const groups = useRef<(THREE.Group | null)[]>([]);
  // One stage per number of rows out, and only the one in use shown.
  useFrame(() => {
    const out = sim.chairRowsOut();
    groups.current.forEach((g, i) => {
      if (g) g.visible = i === out - 1;
    });
  });
  return (
    <>
      {pitch.stages.map((st, i) => (
        <group
          key={i}
          ref={(g) => {
            groups.current[i] = g;
          }}
          visible={i === 0}
        >
          <Boards stage={st} env={env} />
        </group>
      ))}
      {pitch.chairs.map((p, i) => (
        <Chair key={i} at={p} row={pitch.chairRows[i]} sim={sim} />
      ))}
      {pitch.crowd.map((p, i) => (
        <Onlooker key={i} at={p} seed={i + 1} />
      ))}
    </>
  );
}

export function Scenery({
  stage,
  env,
  pitching,
  sim,
}: {
  stage: Stage;
  env: Environment;
  /** Whether anyone is Shipping and waiting, which is when the stage is up. */
  pitching: boolean;
  sim: Sim;
}) {
  const { lounge, workshop, pitch, lake } = stage.layout;
  const decor = useMemo(
    () => (["left", "right"] as const).map((side) => scatter(stage, side)),
    [stage],
  );
  return (
    <>
      <hemisphereLight args={[env.sky, env.hemiGround, env.hemiIntensity]} />
      <directionalLight
        position={[6, 12, 8]}
        intensity={env.sunIntensity}
        color={env.sun}
      />
      <mesh rotation-x={-Math.PI / 2}>
        <planeGeometry args={[400, 400]} />
        <meshStandardMaterial color={env.ground} />
      </mesh>
      {decor.map((d, i) => (
        <group key={i}>
          {d.patches.map((p, j) => (
            <mesh
              key={`p${j}`}
              rotation-x={-Math.PI / 2}
              position={[p.x, 0.003, p.z]}
            >
              <circleGeometry args={[p.r, 9]} />
              <meshStandardMaterial color={env.patch} />
            </mesh>
          ))}
          {d.trees.map((t, j) => (
            <Tree
              key={`t${j}`}
              x={t.x}
              z={t.z}
              s={t.s}
              color={env.foliage[t.c]}
              trunk={env.trunk}
            />
          ))}
          {d.rocks.map((r, j) => (
            <mesh
              key={`r${j}`}
              position={[r.x, r.s * 0.4, r.z]}
              rotation-y={r.r}
              scale={[r.s, r.s * 0.7, r.s]}
            >
              <dodecahedronGeometry args={[1, 0]} />
              <meshStandardMaterial color={env.rock} flatShading />
            </mesh>
          ))}
          {d.flowers.map((f, j) => (
            <mesh key={`f${j}`} position={[f.x, 0.05, f.z]}>
              <sphereGeometry args={[0.045, 5, 4]} />
              <meshStandardMaterial
                color={env.flowers[f.c % env.flowers.length]}
              />
            </mesh>
          ))}
          {d.tufts.map((t, j) => (
            <mesh key={`g${j}`} position={[t.x, 0.06, t.z]}>
              <coneGeometry args={[0.05, 0.14, 4]} />
              <meshStandardMaterial
                color={env.foliage[j % env.foliage.length]}
                flatShading
              />
            </mesh>
          ))}
        </group>
      ))}
      {(["left", "right"] as const).map((side) => {
        const at = stage.layout.portals[side];
        // Half turned to face into its back corner by the panel.
        const turn = ((side === "left" ? -1 : 1) * Math.PI) / 4;
        return at ? (
          <Portal key={side} at={at} color={env.portal} turn={turn} />
        ) : null;
      })}
      {lounge && <LoungeArea lounge={lounge} env={env} sim={sim} />}
      {workshop && <WorkshopArea workshop={workshop} />}
      {lake && <LakeArea lake={lake} env={env} sim={sim} />}
      {pitching && pitch && <PitchArea pitch={pitch} env={env} sim={sim} />}
      {env.fireflies && <Fireflies stage={stage} />}
    </>
  );
}
