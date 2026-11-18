import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { GardenArea } from "./Garden";
import { PorchArea } from "./Porch";
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
import type { Palette } from "./palette";
import { useEnvironment, type Environment } from "./environment";
import {
  ATHENS,
  AthensBackdrop,
  AthensGate,
  Column,
  Galley,
  Ruin,
  Tablet,
  TempleKiosk,
} from "./environments/athens";
import {
  Crater,
  Crystal,
  HoloBoard,
  LaunchKiosk,
  Meteors,
  MoonBackdrop,
  MoonGate,
  Shuttle,
  SolarPanel,
} from "./environments/moon";
import {
  BallPit,
  BeanBag,
  CarpetTile,
  Clutter,
  Elevator,
  Koi,
  Mug,
  MugTable,
  OfficeBackdrop,
  PottedPlant,
  Reception,
  RubberDuck,
  SlideScreen,
  StickyNote,
} from "./environments/office";

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
  const { lounge, workshop, porch, pitch, lake, houses } = stage.layout;
  const zones = [
    ...houses.map(({ front }) => ({
      xMin: front.x - 1.2,
      xMax: front.x + 1.2,
      zMin: front.z - 1.8,
      zMax: front.z + 0.6,
    })),
    lounge?.area,
    workshop?.area,
    porch?.area,
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
  environment,
}: {
  at: Vec2;
  color: string;
  turn: number;
  environment: Environment;
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
      {environment === "moon" ? (
        <MoonGate color={color} />
      ) : environment === "athens" ? (
        <AthensGate />
      ) : environment === "office" ? (
        <Elevator color={color} />
      ) : (
        <StoneRing color={color} />
      )}
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

function StoneRing({ color }: { color: string }) {
  return (
    <>
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
    </>
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
const CHAIR: Record<Environment, string> = {
  meadow: "#7a5236",
  moon: "#d3d7df",
  athens: "#8a6a4a",
  office: "#3a3f4a",
};

function Chair({
  at,
  row,
  sim,
  wood,
}: {
  at: Vec2;
  row: number;
  sim: Sim;
  wood: string;
}) {
  const group = useRef<THREE.Group>(null);
  useFrame(() => {
    if (!group.current) return;
    group.current.visible = row < sim.chairRowsOut();
  });
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

/** A lounger's frame and cushion, where it isn't a bean bag. */
const LOUNGER: Record<Environment, [string, string]> = {
  meadow: ["#ffffff", "#5bb8d6"],
  moon: ["#d3d7df", "#ff8a3d"],
  athens: ["#8a6a4a", "#c0533f"],
  office: ["#ffffff", "#5bb8d6"],
};

/** A reclining sun lounger, its sitter's feet at `at` and its back behind. */
function Lounger({ at, environment }: { at: Seat; environment: Environment }) {
  const [frame, cushion] = LOUNGER[environment];
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
  const environment = useEnvironment();
  if (environment === "office") return <Mug color={juice} />;
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
function Parasol({
  at,
  seed,
  environment,
}: {
  at: Vec2;
  seed: number;
  environment: Environment;
}) {
  const colors =
    environment === "athens"
      ? ["#2f6fd0", "#ffffff"]
      : seed % 2
        ? ["#ff6f61", "#ffffff"]
        : ["#3fa9f5", "#ffffff"];
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
/** The edge round the pool. */
const COPING: Record<Environment, string> = {
  meadow: "#ffffff",
  moon: "#d3d7df",
  athens: "#f4efe3",
  office: "#3d7bd9",
};

function LoungeArea({
  lounge,
  palette,
  sim,
  environment,
}: {
  lounge: Lounge;
  palette: Palette;
  sim: Sim;
  environment: Environment;
}) {
  const office = environment === "office";
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
        palette.waterGlow * (1 + Math.sin(t * 1.2) * 0.15);
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
        <boxGeometry args={[w + 0.24, office ? 0.12 : 0.05, d + 0.24]} />
        <meshStandardMaterial color={COPING[environment]} flatShading />
      </mesh>
      {office ? (
        <group position={[cx, 0, cz]}>
          <BallPit w={w} d={d} />
        </group>
      ) : (
        <mesh rotation-x={-Math.PI / 2} position={[cx, 0.052, cz]}>
          <planeGeometry args={[w, d]} />
          <meshStandardMaterial
            ref={water}
            color={palette.water}
            emissive={palette.water}
            emissiveIntensity={palette.waterGlow}
          />
        </mesh>
      )}
      <group ref={glints} position={[cx, 0.056, cz]} visible={!office}>
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
      {lounge.seats.map((p, i) =>
        office ? (
          <BeanBag key={i} at={p} seed={i} />
        ) : (
          <Lounger key={i} at={p} environment={environment} />
        ),
      )}
      {lounge.tables.map((p, i) =>
        office ? (
          <MugTable key={i} at={p} seed={i} />
        ) : (
          <DrinkTable key={i} at={p} seed={i} />
        ),
      )}
      {lounge.parasols.map((p, i) =>
        environment === "moon" ? (
          <SolarPanel key={i} at={p} />
        ) : office ? (
          <PottedPlant key={i} {...p} s={1.5} palette={palette} />
        ) : (
          <Parasol key={i} at={p} seed={i} environment={environment} />
        ),
      )}
    </>
  );
}

/** A little sailboat, its bow towards +z; a sailor sits amidships. */
/** The boat as moored; a sailor's is scaled to match, less its own size. */
export const BOAT_SCALE = 1.15;

export function Boat() {
  switch (useEnvironment()) {
    case "moon":
      return <Shuttle />;
    case "athens":
      return <Galley />;
    case "office":
      return <RubberDuck />;
    case "meadow":
      return <Sailboat />;
  }
}

function Sailboat() {
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

/** Turned from the viewer towards the line, which runs off to its right. */
const OFFICE_TURN = 0.7;

/** A seaside kiosk, a striped awning over its window. */
function TicketOffice({
  at,
  environment,
}: {
  at: Vec2;
  environment: Environment;
}) {
  if (environment !== "meadow") {
    return (
      <group position={[at.x, 0, at.z]} rotation-y={OFFICE_TURN}>
        {environment === "moon" ? (
          <LaunchKiosk />
        ) : environment === "athens" ? (
          <TempleKiosk />
        ) : (
          <Reception />
        )}
      </group>
    );
  }
  const wall = "#f2e6cf";
  const trim = "#c0533f";
  return (
    <group position={[at.x, 0, at.z]} rotation-y={OFFICE_TURN}>
      <mesh position-y={0.4}>
        <boxGeometry args={[0.7, 0.8, 0.55]} />
        <meshStandardMaterial color={wall} flatShading />
      </mesh>
      {/* The window, dark inside, with a counter under it. */}
      <mesh position={[0, 0.52, 0.276]}>
        <planeGeometry args={[0.44, 0.26]} />
        <meshBasicMaterial color="#3a2a20" />
      </mesh>
      <mesh position={[0, 0.38, 0.32]}>
        <boxGeometry args={[0.5, 0.03, 0.1]} />
        <meshStandardMaterial color="#a77b50" flatShading />
      </mesh>
      {/* The awning, red and white stripes tipped out over the window. */}
      <group position={[0, 0.72, 0.36]} rotation-x={0.5}>
        {[0, 1, 2, 3, 4].map((i) => (
          <mesh key={i} position-x={-0.24 + i * 0.12}>
            <boxGeometry args={[0.12, 0.02, 0.22]} />
            <meshStandardMaterial
              color={i % 2 ? "#ffffff" : trim}
              flatShading
            />
          </mesh>
        ))}
      </group>
      <mesh position-y={0.83}>
        <boxGeometry args={[0.8, 0.06, 0.65]} />
        <meshStandardMaterial color={trim} flatShading />
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
 * Water running off the window's edge with a sandy rim, lily pads, reeds, a
 * pier out into it with a boat waiting at the end, and a ticket office.
 */
/** The lake's rim, and its pier's planks and posts. */
const SHORE: Record<Environment, { rim: string; deck: string; post: string }> =
  {
    meadow: { rim: "#e3cf9a", deck: "#a77b50", post: "#7a5236" },
    moon: { rim: "#a9a8b0", deck: "#9aa1ad", post: "#5d6370" },
    athens: { rim: "#e8d6a6", deck: "#d8d2c4", post: "#b9b2a2" },
    office: { rim: "#c3c8d0", deck: "#c9a77c", post: "#8a6a4a" },
  };

function LakeArea({
  lake,
  palette,
  sim,
  environment,
}: {
  lake: Lake;
  palette: Palette;
  sim: Sim;
  environment: Environment;
}) {
  const shore = SHORE[environment];
  // Lily pads and reeds where there'd be any.
  const green = environment !== "moon";
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
        palette.waterGlow * 0.5 * (1 + Math.sin(t * 0.9) * 0.15);
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
  // Planks on past the shore under the whole line for the boat.
  const walkEnd = Math.max(
    pierFoot.x,
    (lake.dock[lake.dock.length - 1]?.x ?? 0) + 0.45,
  );
  return (
    <>
      <mesh
        rotation-x={-Math.PI / 2}
        position={[cx, 0.006, cz]}
        scale={[rx, rz, 1]}
      >
        <shapeGeometry args={[sand]} />
        <meshStandardMaterial color={shore.rim} />
      </mesh>
      <mesh
        rotation-x={-Math.PI / 2}
        position={[cx, 0.012, cz]}
        scale={[rx, rz, 1]}
      >
        <shapeGeometry args={[shoreline]} />
        <meshStandardMaterial
          ref={water}
          color={palette.water}
          emissive={palette.water}
          emissiveIntensity={palette.waterGlow * 0.5}
        />
      </mesh>
      {environment === "office" && <Koi cx={cx} cz={cz} rx={rx} rz={rz} />}
      {green && (
        <>
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
              position={[
                cx + Math.sqrt(1 - f * f) * rx - 0.1,
                0.18,
                cz + f * rz,
              ]}
              rotation-z={(i % 3) * 0.12 - 0.1}
            >
              <cylinderGeometry args={[0.012, 0.016, 0.36, 4]} />
              <meshStandardMaterial color="#6f9a3c" flatShading />
            </mesh>
          ))}
        </>
      )}
      {/* The pier: planks on posts, from the shore out over the water. */}
      <mesh position={[(walkEnd + pierEnd.x - 0.15) / 2, 0.07, pierEnd.z]}>
        <boxGeometry args={[walkEnd - pierEnd.x + 0.15, 0.04, 0.42]} />
        <meshStandardMaterial color={shore.deck} flatShading />
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
            <meshStandardMaterial color={shore.post} flatShading />
          </mesh>
        )),
      )}
      <TicketOffice at={lake.office} environment={environment} />
      {/* Moored alongside the pier's end, bow out to the open water. */}
      <group position={[lake.berth.x, 0.02, lake.berth.z]}>
        <group ref={boat} rotation-y={-Math.PI / 2} scale={BOAT_SCALE}>
          <Boat />
        </group>
      </group>
    </>
  );
}

/** Two whiteboards along the back, and the garden. */
function WorkshopArea({
  workshop,
  sim,
  environment,
}: {
  workshop: Workshop;
  sim: Sim;
  environment: Environment;
}) {
  return (
    <>
      {workshop.boards.map((p, i) => (
        <group key={i} position={[p.x, 0, p.z - 0.5]}>
          {environment === "moon" ? (
            <HoloBoard />
          ) : environment === "athens" ? (
            <Tablet />
          ) : (
            <Whiteboard />
          )}
        </group>
      ))}
      <GardenArea garden={workshop.garden} sim={sim} />
    </>
  );
}

function Whiteboard() {
  return (
    <>
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
    </>
  );
}

/** The stage's boards and the seams between them. */
const PLATFORM: Record<Environment, [string, string]> = {
  meadow: ["#b98a5e", "#9a7049"],
  moon: ["#9aa1ad", "#7d838f"],
  athens: ["#efe9dc", "#d8d0bd"],
  office: ["#3a3f4a", "#2f343e"],
};

/** The stage itself: boards on a low platform, and dressing across the back. */
function Boards({
  stage,
  palette,
  environment,
}: {
  stage: Region;
  palette: Palette;
  environment: Environment;
}) {
  const w = stage.xMax - stage.xMin;
  const d = stage.zMax - stage.zMin;
  const cx = (stage.xMin + stage.xMax) / 2;
  const cz = (stage.zMin + stage.zMax) / 2;
  const [boards, seams] = PLATFORM[environment];
  return (
    <>
      <mesh position={[cx, STAGE_HEIGHT / 2, cz]}>
        <boxGeometry args={[w, STAGE_HEIGHT, d]} />
        <meshStandardMaterial color={boards} flatShading />
      </mesh>
      {/* Board seams, so it reads as a stage rather than a slab. */}
      {Array.from({ length: Math.floor(d / 0.5) }, (_, i) => (
        <mesh
          key={i}
          position={[cx, STAGE_HEIGHT + 0.002, stage.zMin + 0.5 * (i + 1)]}
        >
          <boxGeometry args={[w, 0.004, 0.03]} />
          <meshBasicMaterial color={seams} />
        </mesh>
      ))}
      {environment === "office" ? (
        <group position={[cx, 0, stage.zMin + 0.1]}>
          <SlideScreen width={w} />
        </group>
      ) : (
        <Bunting stage={stage} palette={palette} environment={environment} />
      )}
    </>
  );
}

/**
 * Two posts at the back of the stage with something strung between: flags,
 * lights on the moon, laurel between columns in Athens.
 */
function Bunting({
  stage,
  palette,
  environment,
}: {
  stage: Region;
  palette: Palette;
  environment: Environment;
}) {
  const w = stage.xMax - stage.xMin;
  const count = Math.max(
    4,
    Math.round(w / (environment === "meadow" ? 0.45 : 0.25)),
  );
  return (
    <>
      {[stage.xMin + 0.1, stage.xMax - 0.1].map((x) =>
        environment === "athens" ? (
          <group key={x} position={[x, 0, stage.zMin + 0.1]}>
            <Column height={1.5} radius={0.08} />
          </group>
        ) : (
          <mesh key={x} position={[x, 0.75, stage.zMin + 0.1]}>
            <cylinderGeometry args={[0.04, 0.05, 1.5, 6]} />
            <meshStandardMaterial color={palette.trunk} flatShading />
          </mesh>
        ),
      )}
      {Array.from({ length: count }, (_, i) => {
        const f = (i + 0.5) / count;
        const sag = Math.sin(f * Math.PI) * 0.25;
        const at: [number, number, number] = [
          stage.xMin + 0.1 + f * (w - 0.2),
          1.38 - sag,
          stage.zMin + 0.1,
        ];
        if (environment === "moon") {
          const color = palette.flowers[i % palette.flowers.length];
          return (
            <mesh key={i} position={at}>
              <sphereGeometry args={[0.045, 6, 5]} />
              <meshBasicMaterial color={color} toneMapped={false} />
            </mesh>
          );
        }
        if (environment === "athens") {
          return (
            <mesh
              key={i}
              position={at}
              rotation-z={i % 2 ? 0.5 : -0.5}
              scale={[0.6, 1.6, 0.4]}
            >
              <sphereGeometry args={[0.06, 5, 4]} />
              <meshStandardMaterial color={ATHENS.LAUREL} flatShading />
            </mesh>
          );
        }
        return (
          <mesh key={i} position={at} rotation-z={Math.PI}>
            <coneGeometry args={[0.1, 0.2, 3]} />
            <meshStandardMaterial
              color={palette.flowers[i % palette.flowers.length]}
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
  palette,
  sim,
  environment,
}: {
  pitch: Pitch;
  palette: Palette;
  sim: Sim;
  environment: Environment;
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
          <Boards stage={st} palette={palette} environment={environment} />
        </group>
      ))}
      {pitch.chairs.map((p, i) => (
        <Chair
          key={i}
          at={p}
          row={pitch.chairRows[i]}
          sim={sim}
          wood={CHAIR[environment]}
        />
      ))}
      {pitch.crowd.map((p, i) => (
        <Onlooker key={i} at={p} seed={i + 1} />
      ))}
    </>
  );
}

/** What lines the back and outer edge, where a tree would be. */
function Backdrop({
  environment,
  ...props
}: {
  environment: Environment;
  x: number;
  z: number;
  s: number;
  c: number;
  palette: Palette;
}) {
  switch (environment) {
    case "moon":
      return <MoonBackdrop {...props} />;
    case "athens":
      return <AthensBackdrop {...props} />;
    case "office":
      return <OfficeBackdrop {...props} />;
    case "meadow":
      return (
        <Tree
          {...props}
          color={props.palette.foliage[props.c]}
          trunk={props.palette.trunk}
        />
      );
  }
}

/** The odd patch of different ground. */
function Patch({
  environment,
  x,
  z,
  r,
  palette,
}: {
  environment: Environment;
  x: number;
  z: number;
  r: number;
  palette: Palette;
}) {
  if (environment === "moon") {
    return <Crater x={x} z={z} r={r} palette={palette} />;
  }
  if (environment === "office") {
    return <CarpetTile x={x} z={z} r={r} palette={palette} />;
  }
  return (
    <mesh rotation-x={-Math.PI / 2} position={[x, 0.003, z]}>
      <circleGeometry args={[r, 9]} />
      <meshStandardMaterial color={palette.patch} />
    </mesh>
  );
}

function Rock({
  environment,
  ...props
}: {
  environment: Environment;
  x: number;
  z: number;
  s: number;
  r: number;
  palette: Palette;
}) {
  if (environment === "athens") return <Ruin {...props} />;
  if (environment === "office") return <Clutter {...props} />;
  const { x, z, s, r, palette } = props;
  return (
    <mesh position={[x, s * 0.4, z]} rotation-y={r} scale={[s, s * 0.7, s]}>
      <dodecahedronGeometry args={[1, 0]} />
      <meshStandardMaterial color={palette.rock} flatShading />
    </mesh>
  );
}

/** Whatever's dotted about: a flower, a crystal, a sticky note. */
function Speck({
  environment,
  ...props
}: {
  environment: Environment;
  x: number;
  z: number;
  c: number;
  j: number;
  palette: Palette;
}) {
  if (environment === "moon") {
    return props.j % 3 ? null : <Crystal {...props} />;
  }
  if (environment === "office") return <StickyNote {...props} />;
  const { x, z, c, palette } = props;
  return (
    <mesh position={[x, 0.05, z]}>
      <sphereGeometry args={[0.045, 5, 4]} />
      <meshStandardMaterial
        color={palette.flowers[c % palette.flowers.length]}
      />
    </mesh>
  );
}

export function Scenery({
  stage,
  palette,
  pitching,
  sim,
}: {
  stage: Stage;
  palette: Palette;
  /** Whether anyone is Shipping and waiting, which is when the stage is up. */
  pitching: boolean;
  sim: Sim;
}) {
  const environment = useEnvironment();
  const { lounge, workshop, porch, pitch, lake } = stage.layout;
  const decor = useMemo(
    () => (["left", "right"] as const).map((side) => scatter(stage, side)),
    [stage],
  );
  // Grass grows only where there's grass.
  const tufts = environment === "meadow" || environment === "athens";
  return (
    <>
      <hemisphereLight
        args={[palette.sky, palette.hemiGround, palette.hemiIntensity]}
      />
      <directionalLight
        position={[6, 12, 8]}
        intensity={palette.sunIntensity}
        color={palette.sun}
      />
      <mesh rotation-x={-Math.PI / 2}>
        <planeGeometry args={[400, 400]} />
        <meshStandardMaterial color={palette.ground} />
      </mesh>
      {decor.map((d, i) => (
        <group key={i}>
          {d.patches.map((p, j) => (
            <Patch
              key={`p${j}`}
              environment={environment}
              {...p}
              palette={palette}
            />
          ))}
          {d.trees.map((t, j) => (
            <Backdrop
              key={`t${j}`}
              environment={environment}
              {...t}
              palette={palette}
            />
          ))}
          {d.rocks.map((r, j) => (
            <Rock
              key={`r${j}`}
              environment={environment}
              {...r}
              palette={palette}
            />
          ))}
          {d.flowers.map((f, j) => (
            <Speck
              key={`f${j}`}
              environment={environment}
              {...f}
              j={j}
              palette={palette}
            />
          ))}
          {tufts &&
            d.tufts.map((t, j) => (
              <mesh key={`g${j}`} position={[t.x, 0.06, t.z]}>
                <coneGeometry args={[0.05, 0.14, 4]} />
                <meshStandardMaterial
                  color={palette.foliage[j % palette.foliage.length]}
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
          <Portal
            key={side}
            at={at}
            color={palette.portal}
            turn={turn}
            environment={environment}
          />
        ) : null;
      })}
      {lounge && (
        <LoungeArea
          lounge={lounge}
          palette={palette}
          sim={sim}
          environment={environment}
        />
      )}
      {workshop && (
        <WorkshopArea workshop={workshop} sim={sim} environment={environment} />
      )}
      {porch && <PorchArea porch={porch} sim={sim} />}
      {lake && (
        <LakeArea
          lake={lake}
          palette={palette}
          sim={sim}
          environment={environment}
        />
      )}
      {pitching && pitch && (
        <PitchArea
          pitch={pitch}
          palette={palette}
          sim={sim}
          environment={environment}
        />
      )}
      {palette.fireflies && <Fireflies stage={stage} />}
      {environment === "moon" && <Meteors stage={stage} />}
    </>
  );
}
