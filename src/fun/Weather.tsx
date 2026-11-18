import { useMemo, useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { PITCH, type Stage } from "./stage";
import type { Weather } from "./forecast";

/** Deterministic, so screenshots of the same moment match. */
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

/** How high anything falls from. */
const CEILING = 6;
/**
 * Up the screen, height looks like ground further back: something `y` up
 * over ground `z` is drawn where ground `z - y * LEAN` is.
 */
const LEAN = 1 / Math.tan(PITCH);

interface Fall {
  /** Most drops or flakes at once, at a rate of 1. */
  most: number;
  /** Units a second straight down. */
  speed: number;
  /** How much of the wind's m/s it's carried across by, in units a second. */
  carried: number;
  geometry: THREE.BufferGeometry;
  color: string;
  opacity: number;
}

const RAIN: Fall = {
  most: 1100,
  speed: 11,
  carried: 0.35,
  geometry: new THREE.BoxGeometry(0.03, 0.5, 0.03),
  color: "#e2eefa",
  opacity: 0.5,
};

const SNOW: Fall = {
  most: 900,
  speed: 0.9,
  carried: 0.2,
  geometry: new THREE.IcosahedronGeometry(0.045, 0),
  color: "#ffffff",
  opacity: 0.95,
};

const MOST_SPLASHES = 160;
const SPLASH_S = 0.28;
/** A ring lying on the ground. */
const FLAT = new THREE.Quaternion().setFromAxisAngle(
  new THREE.Vector3(1, 0, 0),
  -Math.PI / 2,
);

/** The meadows either side of the panel, by width, to put things over. */
function meadowX(stage: Stage, rng: () => number): number {
  const v = stage.visible;
  const left = stage.panel.xMin - v.xMin;
  const right = v.xMax - stage.panel.xMax;
  const at = rng() * (left + right);
  return at < left ? v.xMin + at : stage.panel.xMax + (at - left);
}

/**
 * Rain or snow over both meadows. Each drop lands on ground spread evenly
 * down the screen and some way below it, which keeps them even on screen at
 * every height; rain splashes where it lands in view.
 */
function Precipitation({
  stage,
  fall,
  wind,
}: {
  stage: Stage;
  fall: NonNullable<Weather["fall"]>;
  wind: Weather["wind"];
}) {
  const kind = fall.kind === "rain" ? RAIN : SNOW;
  const drops = useRef<THREE.InstancedMesh>(null);
  const splashes = useRef<THREE.InstancedMesh>(null);
  const state = useMemo(() => {
    const rng = mulberry32(7);
    const v = stage.visible;
    const land = () => ({
      x: meadowX(stage, rng),
      z: v.zMin + rng() * (v.zMax - v.zMin + CEILING * LEAN),
    });
    return {
      rng,
      land,
      drops: Array.from({ length: kind.most }, () => ({
        ...land(),
        y: rng() * CEILING,
        phase: rng() * Math.PI * 2,
      })),
      splashes: Array.from({ length: MOST_SPLASHES }, () => ({
        x: 0,
        z: 0,
        age: Infinity,
      })),
      next: 0,
    };
  }, [stage, kind]);
  const m = useMemo(() => new THREE.Matrix4(), []);
  const q = useMemo(() => new THREE.Quaternion(), []);
  const p = useMemo(() => new THREE.Vector3(), []);
  const s = useMemo(() => new THREE.Vector3(1, 1, 1), []);
  const up = useMemo(() => new THREE.Vector3(0, 1, 0), []);
  const heading = useMemo(() => new THREE.Vector3(), []);
  useFrame((frame, delta) => {
    const mesh = drops.current;
    if (!mesh) return;
    // A long frame, back from hidden or by a manual clock, isn't a long fall.
    const dt = Math.min(delta, 0.1);
    const t = frame.clock.elapsedTime;
    const count = Math.round(kind.most * Math.min(1, fall.rate));
    const vx = wind.x * kind.carried;
    const vz = wind.z * kind.carried;
    // Rain streaks lean along the way they're falling; flakes tumble.
    heading.set(vx, kind.speed, vz).normalize();
    q.setFromUnitVectors(up, heading);
    const rain = fall.kind === "rain";
    for (let i = 0; i < count; i++) {
      const d = state.drops[i];
      d.y -= kind.speed * dt;
      d.x += vx * dt;
      d.z += vz * dt;
      if (d.y < 0) {
        if (rain && d.z < stage.visible.zMax) {
          const sp = state.splashes[state.next];
          state.next = (state.next + 1) % MOST_SPLASHES;
          sp.x = d.x;
          sp.z = d.z;
          sp.age = 0;
        }
        Object.assign(d, state.land());
        d.y += CEILING;
      }
      const sway = rain ? 0 : Math.sin(t * 1.3 + d.phase) * 0.25;
      p.set(d.x + sway, d.y, d.z + (rain ? 0 : Math.cos(t + d.phase) * 0.1));
      if (!rain) q.setFromAxisAngle(up, t + d.phase);
      mesh.setMatrixAt(i, m.compose(p, q, s));
    }
    mesh.count = count;
    mesh.instanceMatrix.needsUpdate = true;

    const rings = splashes.current;
    if (!rings) return;
    let shown = 0;
    q.copy(FLAT);
    for (const sp of state.splashes) {
      sp.age += dt;
      if (sp.age >= SPLASH_S) continue;
      const r = 0.6 + (sp.age / SPLASH_S) * 2;
      p.set(sp.x, 0.025, sp.z);
      rings.setMatrixAt(shown++, m.compose(p, q, s.setScalar(r)));
    }
    s.setScalar(1);
    rings.count = shown;
    rings.instanceMatrix.needsUpdate = true;
  });
  return (
    <>
      <instancedMesh
        ref={drops}
        args={[kind.geometry, undefined, kind.most]}
        frustumCulled={false}
      >
        <meshBasicMaterial
          color={kind.color}
          transparent
          opacity={kind.opacity}
          depthWrite={false}
        />
      </instancedMesh>
      {fall.kind === "rain" && (
        <instancedMesh
          ref={splashes}
          args={[undefined, undefined, MOST_SPLASHES]}
          frustumCulled={false}
        >
          <torusGeometry args={[0.04, 0.012, 3, 12]} />
          <meshBasicMaterial
            color="#e4eef8"
            transparent
            opacity={0.6}
            depthWrite={false}
          />
        </instancedMesh>
      )}
    </>
  );
}

/** A soft round shadow, dark in the middle and gone at the edge. */
function softShadow(): THREE.Texture {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 128;
  const ctx = canvas.getContext("2d")!;
  const g = ctx.createRadialGradient(64, 64, 0, 64, 64, 64);
  g.addColorStop(0, "rgba(0,0,0,1)");
  g.addColorStop(0.5, "rgba(0,0,0,0.75)");
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 128, 128);
  return new THREE.CanvasTexture(canvas);
}

/**
 * Shadows of broken cloud drifting over the meadows with the wind. A clear
 * sky has none, and an overcast one is shade all over, not shadows.
 */
function CloudShadows({ stage, weather }: { stage: Stage; weather: Weather }) {
  const map = useMemo(softShadow, []);
  const group = useRef<THREE.Group>(null);
  const clouds = useMemo(() => {
    const rng = mulberry32(11);
    const v = stage.visible;
    return Array.from({ length: 9 }, () => ({
      x: v.xMin + rng() * (v.xMax - v.xMin),
      z: v.zMin + rng() * (v.zMax - v.zMin),
      w: 5 + rng() * 6,
      d: 3 + rng() * 4,
    }));
  }, [stage]);
  const broken = Math.sin(Math.PI * weather.clouds);
  const shown = Math.round(weather.clouds * clouds.length);
  useFrame((_, delta) => {
    const dt = Math.min(delta, 0.1);
    const v = stage.visible;
    const speed = Math.hypot(weather.wind.x, weather.wind.z);
    // Cloud goes by even on a still day down here.
    const drift = Math.max(0.25, speed * 0.12);
    const dx = speed > 0.1 ? (weather.wind.x / speed) * drift : drift;
    const dz = speed > 0.1 ? (weather.wind.z / speed) * drift : 0;
    group.current?.children.forEach((child, i) => {
      const c = clouds[i];
      c.x += dx * dt;
      c.z += dz * dt;
      // Round again from the far side once wholly gone.
      const spanX = v.xMax - v.xMin + c.w * 2;
      const spanZ = v.zMax - v.zMin + c.d * 2;
      if (c.x > v.xMax + c.w) c.x -= spanX;
      if (c.x < v.xMin - c.w) c.x += spanX;
      if (c.z > v.zMax + c.d) c.z -= spanZ;
      if (c.z < v.zMin - c.d) c.z += spanZ;
      child.position.set(c.x, 0.02, c.z);
    });
  });
  if (broken < 0.05) return null;
  return (
    <group ref={group}>
      {clouds.slice(0, shown).map((c, i) => (
        <mesh
          key={i}
          rotation-x={-Math.PI / 2}
          position={[c.x, 0.02, c.z]}
          scale={[c.w, c.d, 1]}
        >
          <planeGeometry />
          <meshBasicMaterial
            map={map}
            color="#000000"
            transparent
            opacity={0.24 * broken}
            depthWrite={false}
          />
        </mesh>
      ))}
    </group>
  );
}

/** A flash of lightning now and then: a flicker, and sometimes another. */
function Lightning() {
  const light = useRef<THREE.AmbientLight>(null);
  const rng = useMemo(() => mulberry32(3), []);
  const plan = useRef({ next: 4, flashes: [] as number[] });
  useFrame((frame) => {
    const t = frame.clock.elapsedTime;
    const p = plan.current;
    if (t >= p.next) {
      p.flashes = [t, t + 0.12];
      if (rng() < 0.5) p.flashes.push(t + 0.3 + rng() * 0.2);
      p.next = t + 6 + rng() * 12;
    }
    let level = 0;
    for (const at of p.flashes) {
      const since = t - at;
      if (since >= 0 && since < 0.18) level = Math.max(level, 1 - since / 0.18);
    }
    if (light.current) light.current.intensity = level * 2.6;
  });
  return <ambientLight ref={light} color="#eef2ff" intensity={0} />;
}

/** How far from the camera the fog starts, and where it closes in. */
const FOG_NEAR = 30;
const FOG_FAR = 62;

/** Whatever the weather is doing over the meadows. */
export function WeatherLayer({
  stage,
  weather,
  sky,
}: {
  stage: Stage;
  weather: Weather;
  /** The fog takes the sky's colour. */
  sky: string;
}) {
  return (
    <>
      {weather.fog && <fog attach="fog" args={[sky, FOG_NEAR, FOG_FAR]} />}
      <CloudShadows stage={stage} weather={weather} />
      {weather.fall && weather.fall.rate > 0 && (
        <Precipitation stage={stage} fall={weather.fall} wind={weather.wind} />
      )}
      {weather.storm && <Lightning />}
    </>
  );
}
