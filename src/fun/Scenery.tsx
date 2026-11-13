import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import type { Lounge, Pitch, Side, Sim, Vec2, Workshop } from "./sim";
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
  /** The lounge's bare-earth clearing. */
  clearing: string;
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
  clearing: "#d9c08f",
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
  clearing: "#6b5a43",
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

  // A treeline along the back, leaving the portal clear.
  for (let x = xMin + 0.3; x < xMax; x += 0.7 + rng() * 0.6) {
    if (portal && Math.abs(x - portal.x) < 1.1) continue;
    decor.trees.push({
      x,
      z: v.zMin + 0.4 + rng() * (region.zMin - v.zMin - 0.6),
      s: 0.8 + rng() * 0.6,
      c: Math.floor(rng() * 3),
    });
  }
  // And a few down the outer edge.
  const outer = side === "left" ? xMin + 0.35 : xMax - 0.35;
  for (let z = region.zMin + 1; z < v.zMax; z += 1.6 + rng() * 1.8) {
    decor.trees.push({
      x: outer + (rng() - 0.5) * 0.3,
      z,
      s: 0.6 + rng() * 0.5,
      c: Math.floor(rng() * 3),
    });
  }
  const area = (region.xMax - region.xMin) * (region.zMax - region.zMin);
  const point = () => ({
    x: region.xMin + rng() * (region.xMax - region.xMin),
    z: region.zMin + rng() * (region.zMax - region.zMin),
  });
  // Rocks keep out of the zones, where they'd sit under someone's feet.
  const zones = [stage.layout.lounge?.area, stage.layout.workshop?.area];
  const inZone = (p: Vec2) =>
    side === "right" &&
    zones.some(
      (z) => z && p.x > z.xMin && p.x < z.xMax && p.z > z.zMin && p.z < z.zMax,
    );
  for (let i = 0; i < area * 0.04; i++) {
    const p = point();
    if (inZone(p)) continue;
    decor.rocks.push({ ...p, s: 0.12 + rng() * 0.16, r: rng() * Math.PI });
  }
  for (let i = 0; i < area * 0.35; i++)
    decor.flowers.push({ ...point(), c: Math.floor(rng() * 4) });
  for (let i = 0; i < area * 0.3; i++) decor.tufts.push(point());
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

function Portal({ at, color }: { at: Vec2; color: string }) {
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
    <group position={[at.x, 0, at.z - 0.15]}>
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
function Chair({ at, index, sim }: { at: Vec2; index: number; sim: Sim }) {
  const group = useRef<THREE.Group>(null);
  useFrame(() => {
    if (group.current) group.current.visible = sim.chairTaken(index);
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

/** A dirt clearing with a campfire, log seats round it and bedrolls about. */
function LoungeArea({ lounge, env }: { lounge: Lounge; env: Environment }) {
  const flames = useRef<THREE.Group>(null);
  const light = useRef<THREE.PointLight>(null);
  useFrame((state) => {
    const t = state.clock.elapsedTime;
    flames.current?.children.forEach((f, i) => {
      f.scale.y = 1 + Math.sin(t * (7 + i * 2.3) + i) * 0.22;
      f.rotation.y = t * (0.6 + i * 0.2);
    });
    if (light.current) light.current.intensity = 2.4 + Math.sin(t * 9) * 0.4;
  });
  const { area, fire } = lounge;
  const rx = (area.xMax - area.xMin) / 2 + 0.3;
  const rz = (area.zMax - area.zMin) / 2;
  return (
    <>
      <mesh
        rotation-x={-Math.PI / 2}
        position={[fire.x, 0.004, (area.zMin + area.zMax) / 2]}
        scale={[rx, rz, 1]}
      >
        <circleGeometry args={[1, 28]} />
        <meshStandardMaterial color={env.clearing} />
      </mesh>
      <group position={[fire.x, 0, fire.z]}>
        {Array.from({ length: 7 }, (_, i) => (
          <mesh
            key={i}
            position={[
              Math.cos((i / 7) * Math.PI * 2) * 0.32,
              0.05,
              Math.sin((i / 7) * Math.PI * 2) * 0.32,
            ]}
          >
            <dodecahedronGeometry args={[0.08, 0]} />
            <meshStandardMaterial color={env.rock} flatShading />
          </mesh>
        ))}
        {[0, 1].map((i) => (
          <mesh key={i} position-y={0.06} rotation={[0, i * 1.4, Math.PI / 2]}>
            <cylinderGeometry args={[0.045, 0.045, 0.5, 6]} />
            <meshStandardMaterial color={env.trunk} flatShading />
          </mesh>
        ))}
        <group ref={flames} position-y={0.1}>
          {["#ff9f1c", "#ffcd3c", "#ff6b35"].map((c, i) => (
            <mesh key={c} position={[(i - 1) * 0.06, 0.12, 0]}>
              <coneGeometry args={[0.1 - i * 0.015, 0.34 - i * 0.05, 5]} />
              <meshBasicMaterial color={c} />
            </mesh>
          ))}
        </group>
        <pointLight
          ref={light}
          position-y={0.5}
          color="#ffb050"
          distance={5}
          decay={1}
        />
      </group>
      {lounge.seats.map((p, i) => {
        // Lengthways round the fire, so whoever sits faces it.
        const angle = Math.atan2(p.x - fire.x, p.z - fire.z);
        return (
          <mesh
            key={i}
            position={[p.x, 0.07, p.z]}
            rotation={[0, angle + Math.PI / 2, Math.PI / 2]}
          >
            <cylinderGeometry args={[0.08, 0.08, 0.5, 7]} />
            <meshStandardMaterial color={env.trunk} flatShading />
          </mesh>
        );
      })}
      {lounge.naps.map((p, i) => (
        <group key={i} position={[p.x, 0, p.z - 0.3]}>
          <mesh position-y={0.025}>
            <boxGeometry args={[0.42, 0.05, 0.85]} />
            <meshStandardMaterial
              color={env.flowers[i % env.flowers.length]}
              flatShading
            />
          </mesh>
          <mesh position={[0, 0.06, -0.32]}>
            <boxGeometry args={[0.3, 0.07, 0.16]} />
            <meshStandardMaterial color="#f4efe4" flatShading />
          </mesh>
        </group>
      ))}
    </>
  );
}

/** A paved yard, two whiteboards along the back, and two piles of crates. */
function WorkshopArea({
  workshop,
  bottom,
}: {
  workshop: Workshop;
  bottom: number;
}) {
  const { area } = workshop;
  const w = area.xMax - area.xMin + 0.6;
  const d = bottom - area.zMin;
  return (
    <>
      <mesh
        rotation-x={-Math.PI / 2}
        position={[(area.xMin + area.xMax) / 2, 0.004, area.zMin + d / 2]}
      >
        <planeGeometry args={[w, d]} />
        <meshStandardMaterial color="#8f8a80" transparent opacity={0.35} />
      </mesh>
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

function PitchArea({
  pitch,
  env,
  sim,
}: {
  pitch: Pitch;
  env: Environment;
  sim: Sim;
}) {
  const { stage, crowd } = pitch;
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
      {pitch.chairs.map((p, i) => (
        <Chair key={i} at={p} index={i} sim={sim} />
      ))}
      {crowd.map((p, i) => (
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
  const { lounge, workshop, pitch } = stage.layout;
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
        return at ? <Portal key={side} at={at} color={env.portal} /> : null;
      })}
      {lounge && <LoungeArea lounge={lounge} env={env} />}
      {workshop && (
        <WorkshopArea
          workshop={workshop}
          bottom={pitching && pitch ? pitch.top : workshop.area.zMax}
        />
      )}
      {pitching && pitch && <PitchArea pitch={pitch} env={env} sim={sim} />}
      {env.fireflies && <Fireflies stage={stage} />}
    </>
  );
}
