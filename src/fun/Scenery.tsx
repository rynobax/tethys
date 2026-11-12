import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import type { Side, Vec2 } from "./sim";
import type { Stage } from "./stage";

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
  const decor: Decor = { trees: [], rocks: [], flowers: [], tufts: [], patches: [] };
  if (!region) return decor;
  const v = stage.visible;
  const xMin = side === "left" ? v.xMin : stage.panel.xMax;
  const xMax = side === "left" ? stage.panel.xMin : v.xMax;
  const rng = mulberry32(Math.round((xMax - xMin) * 10) * 7919 + Math.round((v.zMax - v.zMin) * 10) + (side === "left" ? 1 : 2));
  const portal = stage.layout.portals[side];

  // A treeline along the back, leaving the portal clear.
  for (let x = xMin + 0.3; x < xMax; x += 0.7 + rng() * 0.6) {
    if (portal && Math.abs(x - portal.x) < 1.1) continue;
    decor.trees.push({ x, z: v.zMin + 0.4 + rng() * (region.zMin - v.zMin - 0.6), s: 0.8 + rng() * 0.6, c: Math.floor(rng() * 3) });
  }
  // And a few down the outer edge.
  const outer = side === "left" ? xMin + 0.35 : xMax - 0.35;
  for (let z = region.zMin + 1; z < v.zMax; z += 1.6 + rng() * 1.8) {
    decor.trees.push({ x: outer + (rng() - 0.5) * 0.3, z, s: 0.6 + rng() * 0.5, c: Math.floor(rng() * 3) });
  }
  const area = (region.xMax - region.xMin) * (region.zMax - region.zMin);
  const point = () => ({
    x: region.xMin + rng() * (region.xMax - region.xMin),
    z: region.zMin + rng() * (region.zMax - region.zMin),
  });
  for (let i = 0; i < area * 0.04; i++) decor.rocks.push({ ...point(), s: 0.12 + rng() * 0.16, r: rng() * Math.PI });
  for (let i = 0; i < area * 0.35; i++) decor.flowers.push({ ...point(), c: Math.floor(rng() * 4) });
  for (let i = 0; i < area * 0.3; i++) decor.tufts.push(point());
  for (let i = 0; i < area * 0.03 + 1; i++) decor.patches.push({ ...point(), r: 0.8 + rng() * 1.6 });
  return decor;
}

function Tree({ x, z, s, color, trunk }: { x: number; z: number; s: number; color: string; trunk: string }) {
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
      (swirl.current.material as THREE.MeshBasicMaterial).opacity = 0.55 + Math.sin(t * 2) * 0.15;
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
        <meshStandardMaterial color={color} emissive={color} emissiveIntensity={0.45} flatShading />
      </mesh>
      <mesh ref={swirl} position-y={0.68}>
        <circleGeometry args={[0.5, 6]} />
        <meshBasicMaterial color={color} transparent opacity={0.6} side={THREE.DoubleSide} />
      </mesh>
      <group ref={sparks} position-y={0.68}>
        {[0, 1, 2, 3, 4].map((i) => (
          <mesh key={i} position={[Math.cos((i / 5) * Math.PI * 2) * 0.72, Math.sin((i / 5) * Math.PI * 2) * 0.72, 0]}>
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

export function Scenery({ stage, env }: { stage: Stage; env: Environment }) {
  const decor = useMemo(
    () => (["left", "right"] as const).map((side) => scatter(stage, side)),
    [stage],
  );
  return (
    <>
      <hemisphereLight args={[env.sky, env.hemiGround, env.hemiIntensity]} />
      <directionalLight position={[6, 12, 8]} intensity={env.sunIntensity} color={env.sun} />
      <mesh rotation-x={-Math.PI / 2}>
        <planeGeometry args={[400, 400]} />
        <meshStandardMaterial color={env.ground} />
      </mesh>
      {decor.map((d, i) => (
        <group key={i}>
          {d.patches.map((p, j) => (
            <mesh key={`p${j}`} rotation-x={-Math.PI / 2} position={[p.x, 0.003, p.z]}>
              <circleGeometry args={[p.r, 9]} />
              <meshStandardMaterial color={env.patch} />
            </mesh>
          ))}
          {d.trees.map((t, j) => (
            <Tree key={`t${j}`} x={t.x} z={t.z} s={t.s} color={env.foliage[t.c]} trunk={env.trunk} />
          ))}
          {d.rocks.map((r, j) => (
            <mesh key={`r${j}`} position={[r.x, r.s * 0.4, r.z]} rotation-y={r.r} scale={[r.s, r.s * 0.7, r.s]}>
              <dodecahedronGeometry args={[1, 0]} />
              <meshStandardMaterial color={env.rock} flatShading />
            </mesh>
          ))}
          {d.flowers.map((f, j) => (
            <mesh key={`f${j}`} position={[f.x, 0.05, f.z]}>
              <sphereGeometry args={[0.045, 5, 4]} />
              <meshStandardMaterial color={env.flowers[f.c % env.flowers.length]} />
            </mesh>
          ))}
          {d.tufts.map((t, j) => (
            <mesh key={`g${j}`} position={[t.x, 0.06, t.z]}>
              <coneGeometry args={[0.05, 0.14, 4]} />
              <meshStandardMaterial color={env.foliage[j % env.foliage.length]} flatShading />
            </mesh>
          ))}
        </group>
      ))}
      {(["left", "right"] as const).map((side) => {
        const at = stage.layout.portals[side];
        return at ? <Portal key={side} at={at} color={env.portal} /> : null;
      })}
      {env.fireflies && <Fireflies stage={stage} />}
    </>
  );
}
