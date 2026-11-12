import { useMemo, useRef } from "react";
import { useFrame, type ThreeEvent } from "@react-three/fiber";
import * as THREE from "three";
import type { Resident } from "./residents";
import type { Agent, Sim } from "./sim";
import type {
  Accessory,
  AvatarSpec,
  Body,
  Eyes,
  Hat,
  PropPart,
  Work,
} from "./spec";

/** Where things sit on each body, in units of a top-level resident. */
interface Dims {
  center: number;
  top: number;
  hw: number;
  eyeY: number;
  eyeX: number;
  faceZ: number;
  neck: number;
  shoulder: number;
}

export const DIMS: Record<Body, Dims> = {
  bean: {
    center: 0.61,
    top: 1.1,
    hw: 0.3,
    eyeY: 0.8,
    eyeX: 0.11,
    faceZ: 0.3,
    neck: 0.52,
    shoulder: 0.58,
  },
  tall: {
    center: 0.67,
    top: 1.22,
    hw: 0.25,
    eyeY: 0.94,
    eyeX: 0.095,
    faceZ: 0.25,
    neck: 0.62,
    shoulder: 0.66,
  },
  round: {
    center: 0.54,
    top: 0.96,
    hw: 0.42,
    eyeY: 0.64,
    eyeX: 0.13,
    faceZ: 0.4,
    neck: 0.36,
    shoulder: 0.5,
  },
  blob: {
    center: 0.48,
    top: 0.84,
    hw: 0.46,
    eyeY: 0.56,
    eyeX: 0.14,
    faceZ: 0.44,
    neck: 0.32,
    shoulder: 0.44,
  },
  cube: {
    center: 0.45,
    top: 0.78,
    hw: 0.32,
    eyeY: 0.55,
    eyeX: 0.12,
    faceZ: 0.28,
    neck: 0.3,
    shoulder: 0.45,
  },
};

/** Room above the body a hat can take, for placing labels. */
export const HAT_ROOM = 0.35;

const DEG = Math.PI / 180;
const PARTY_PINK = "#ff5ca8";

function Mat({ color, emissive }: { color: string; emissive?: number }) {
  return (
    <meshStandardMaterial
      color={color}
      flatShading
      roughness={0.75}
      emissive={emissive ? color : "#000000"}
      emissiveIntensity={emissive ?? 0}
    />
  );
}

function BodyMesh({ body, color }: { body: Body; color: string }) {
  const d = DIMS[body];
  switch (body) {
    case "bean":
      return (
        <mesh position-y={d.center}>
          <capsuleGeometry args={[0.3, 0.38, 4, 10]} />
          <Mat color={color} />
        </mesh>
      );
    case "tall":
      return (
        <mesh position-y={d.center}>
          <capsuleGeometry args={[0.25, 0.6, 4, 10]} />
          <Mat color={color} />
        </mesh>
      );
    case "round":
      return (
        <mesh position-y={d.center}>
          <sphereGeometry args={[0.42, 12, 10]} />
          <Mat color={color} />
        </mesh>
      );
    case "blob":
      return (
        <mesh position-y={d.center} scale={[1, 0.78, 1]}>
          <sphereGeometry args={[0.46, 12, 10]} />
          <Mat color={color} />
        </mesh>
      );
    case "cube":
      return (
        <mesh position-y={d.center}>
          <boxGeometry args={[0.64, 0.66, 0.56]} />
          <Mat color={color} />
        </mesh>
      );
  }
}

function EyesMesh({
  eyes,
  d,
  accent,
}: {
  eyes: Eyes;
  d: Dims;
  accent: string;
}) {
  const z = d.faceZ;
  switch (eyes) {
    case "dots":
      return (
        <>
          {[-1, 1].map((s) => (
            <mesh key={s} position={[s * d.eyeX, 0, z - 0.01]}>
              <sphereGeometry args={[0.045, 8, 6]} />
              <Mat color="#1d1d1f" />
            </mesh>
          ))}
        </>
      );
    case "sleepy":
      return (
        <>
          {[-1, 1].map((s) => (
            <mesh
              key={s}
              position={[s * d.eyeX, 0, z - 0.01]}
              scale={[1, 0.35, 0.6]}
            >
              <sphereGeometry args={[0.055, 8, 6]} />
              <Mat color="#1d1d1f" />
            </mesh>
          ))}
        </>
      );
    case "wide":
      return (
        <>
          {[-1, 1].map((s) => (
            <group key={s} position={[s * d.eyeX, 0, z - 0.03]}>
              <mesh>
                <sphereGeometry args={[0.085, 10, 8]} />
                <Mat color="#ffffff" />
              </mesh>
              <mesh position-z={0.06}>
                <sphereGeometry args={[0.045, 8, 6]} />
                <Mat color="#1d1d1f" />
              </mesh>
            </group>
          ))}
        </>
      );
    case "visor":
      return (
        <mesh position-z={z - 0.01}>
          <boxGeometry args={[d.hw * 1.7, 0.12, 0.08]} />
          <Mat color="#1d2433" />
          <mesh position-z={0.045} scale={[0.85, 0.25, 1]}>
            <planeGeometry args={[d.hw * 1.7, 0.12]} />
            <meshBasicMaterial color={accent} />
          </mesh>
        </mesh>
      );
    case "cyclops":
      return (
        <group position-z={z - 0.05}>
          <mesh>
            <sphereGeometry args={[0.13, 12, 10]} />
            <Mat color="#ffffff" />
          </mesh>
          <mesh position-z={0.1}>
            <sphereGeometry args={[0.06, 8, 6]} />
            <Mat color="#1d1d1f" />
          </mesh>
        </group>
      );
  }
}

function HalfSphere({ r, color }: { r: number; color: string }) {
  return (
    <mesh>
      <sphereGeometry args={[r, 12, 6, 0, Math.PI * 2, 0, Math.PI / 2]} />
      <Mat color={color} />
    </mesh>
  );
}

function HatMesh({
  hat,
  d,
  accent,
  propeller,
}: {
  hat: Hat | "party";
  d: Dims;
  accent: string;
  propeller: React.RefObject<THREE.Group | null>;
}) {
  const T = d.top;
  const r = Math.min(d.hw, 0.3) + 0.02;
  switch (hat) {
    case "none":
      return null;
    case "cap":
      return (
        <group position-y={T - 0.07}>
          <HalfSphere r={r} color={accent} />
          <mesh position={[0, 0.01, r * 0.8]}>
            <boxGeometry args={[r * 1.2, 0.03, r * 0.9]} />
            <Mat color={accent} />
          </mesh>
        </group>
      );
    case "beanie":
      return (
        <group position-y={T - 0.08}>
          <HalfSphere r={r + 0.01} color={accent} />
          <mesh position-y={r + 0.04}>
            <sphereGeometry args={[0.075, 8, 6]} />
            <Mat color="#ffffff" />
          </mesh>
        </group>
      );
    case "wizard":
      return (
        <group position-y={T - 0.02} rotation-z={0.15}>
          <mesh>
            <cylinderGeometry args={[0.36, 0.36, 0.03, 14]} />
            <Mat color={accent} />
          </mesh>
          <mesh position-y={0.3}>
            <coneGeometry args={[0.24, 0.6, 8]} />
            <Mat color={accent} />
          </mesh>
          <mesh position={[0.06, 0.25, 0.18]}>
            <octahedronGeometry args={[0.05]} />
            <Mat color="#ffe066" emissive={0.6} />
          </mesh>
        </group>
      );
    case "hardhat":
      return (
        <group position-y={T - 0.07}>
          <HalfSphere r={r + 0.02} color={accent} />
          <mesh position-y={0.01}>
            <cylinderGeometry args={[r + 0.1, r + 0.1, 0.03, 14]} />
            <Mat color={accent} />
          </mesh>
        </group>
      );
    case "crown":
      return (
        <group position-y={T + 0.03}>
          <mesh>
            <cylinderGeometry args={[0.2, 0.2, 0.12, 8, 1, true]} />
            <meshStandardMaterial
              color="#ffcf40"
              flatShading
              side={THREE.DoubleSide}
              metalness={0.3}
              roughness={0.4}
            />
          </mesh>
          {[0, 1, 2, 3, 4].map((i) => (
            <mesh
              key={i}
              position={[
                Math.sin((i / 5) * Math.PI * 2) * 0.2,
                0.1,
                Math.cos((i / 5) * Math.PI * 2) * 0.2,
              ]}
            >
              <coneGeometry args={[0.04, 0.09, 4]} />
              <Mat color="#ffcf40" />
            </mesh>
          ))}
        </group>
      );
    case "antenna":
      return (
        <group position-y={T}>
          <mesh position-y={0.14}>
            <cylinderGeometry args={[0.015, 0.015, 0.28, 5]} />
            <Mat color="#444444" />
          </mesh>
          <mesh position-y={0.3}>
            <sphereGeometry args={[0.06, 8, 6]} />
            <Mat color={accent} emissive={0.5} />
          </mesh>
        </group>
      );
    case "chef":
      return (
        <group position-y={T - 0.02}>
          <mesh position-y={0.09}>
            <cylinderGeometry args={[0.19, 0.19, 0.2, 12]} />
            <Mat color="#ffffff" />
          </mesh>
          <mesh position-y={0.24} scale={[1, 0.7, 1]}>
            <sphereGeometry args={[0.25, 10, 8]} />
            <Mat color="#ffffff" />
          </mesh>
        </group>
      );
    case "headphones": {
      const radius = Math.max(d.hw, d.top - d.eyeY) + 0.03;
      const cy = d.top + 0.03 - radius;
      return (
        <group position-y={cy}>
          <mesh>
            <torusGeometry args={[radius, 0.03, 6, 18, Math.PI]} />
            <Mat color="#2b2b2b" />
          </mesh>
          {[-1, 1].map((s) => (
            <mesh key={s} position-x={s * radius} rotation-z={Math.PI / 2}>
              <cylinderGeometry args={[0.1, 0.1, 0.08, 10]} />
              <Mat color={accent} />
            </mesh>
          ))}
        </group>
      );
    }
    case "propeller":
      return (
        <group position-y={T - 0.06}>
          <HalfSphere r={r} color={accent} />
          <mesh position-y={r + 0.04}>
            <cylinderGeometry args={[0.025, 0.025, 0.1, 6]} />
            <Mat color="#666666" />
          </mesh>
          <group ref={propeller} position-y={r + 0.1}>
            <mesh>
              <boxGeometry args={[0.42, 0.015, 0.07]} />
              <Mat color="#ff6b6b" />
            </mesh>
            <mesh rotation-y={Math.PI / 2}>
              <boxGeometry args={[0.42, 0.015, 0.07]} />
              <Mat color="#4dabf7" />
            </mesh>
          </group>
        </group>
      );
    case "party":
      return (
        <group position-y={T - 0.02} rotation-z={-0.12}>
          <mesh position-y={0.19}>
            <coneGeometry args={[0.15, 0.38, 10]} />
            <Mat color={PARTY_PINK} />
          </mesh>
          <mesh position-y={0.4}>
            <sphereGeometry args={[0.06, 8, 6]} />
            <Mat color="#ffe066" />
          </mesh>
        </group>
      );
  }
}

function AccessoryMesh({
  accessory,
  d,
  accent,
}: {
  accessory: Accessory;
  d: Dims;
  accent: string;
}) {
  switch (accessory) {
    case "none":
      return null;
    case "scarf":
      return (
        <group position-y={d.neck}>
          <mesh rotation-x={Math.PI / 2}>
            <torusGeometry args={[d.hw * 0.97, 0.06, 6, 16]} />
            <Mat color={accent} />
          </mesh>
          <mesh
            position={[d.hw * 0.4, -0.13, d.faceZ * 0.85]}
            rotation-z={0.15}
          >
            <boxGeometry args={[0.1, 0.24, 0.04]} />
            <Mat color={accent} />
          </mesh>
        </group>
      );
    case "glasses":
      return (
        <group position={[0, d.eyeY, d.faceZ + 0.03]}>
          {[-1, 1].map((s) => (
            <mesh key={s} position-x={s * d.eyeX}>
              <torusGeometry args={[0.075, 0.014, 6, 14]} />
              <Mat color="#222222" />
            </mesh>
          ))}
          <mesh>
            <boxGeometry args={[d.eyeX * 2 - 0.15, 0.02, 0.02]} />
            <Mat color="#222222" />
          </mesh>
        </group>
      );
    case "cape":
      return (
        <mesh
          position={[0, d.shoulder - 0.22, -(d.faceZ + 0.03)]}
          rotation-x={0.12}
        >
          <planeGeometry args={[d.hw * 2.1, 0.55]} />
          <meshStandardMaterial
            color={accent}
            flatShading
            side={THREE.DoubleSide}
          />
        </mesh>
      );
    case "backpack":
      return (
        <mesh position={[0, d.center, -(d.faceZ + 0.07)]}>
          <boxGeometry args={[d.hw * 1.3, 0.36, 0.2]} />
          <Mat color={accent} />
        </mesh>
      );
    case "bowtie":
      return (
        <group position={[0, d.neck, d.faceZ + 0.01]}>
          {[-1, 1].map((s) => (
            <mesh key={s} position-x={s * 0.06} rotation-z={(s * Math.PI) / 2}>
              <coneGeometry args={[0.06, 0.12, 4]} />
              <Mat color={accent} />
            </mesh>
          ))}
          <mesh>
            <sphereGeometry args={[0.032, 6, 5]} />
            <Mat color={accent} />
          </mesh>
        </group>
      );
  }
}

function PartMesh({ part }: { part: PropPart }) {
  const geometry = (() => {
    switch (part.shape) {
      case "box":
        return <boxGeometry args={[1, 1, 1]} />;
      case "sphere":
        return <sphereGeometry args={[0.5, 10, 8]} />;
      case "cylinder":
        return <cylinderGeometry args={[0.5, 0.5, 1, 10]} />;
      case "cone":
        return <coneGeometry args={[0.5, 1, 10]} />;
      case "torus":
        return <torusGeometry args={[0.4, 0.1, 8, 16]} />;
    }
  })();
  return (
    <mesh
      position={part.position}
      scale={part.scale}
      rotation={[
        part.rotation[0] * DEG,
        part.rotation[1] * DEG,
        part.rotation[2] * DEG,
      ]}
    >
      {geometry}
      <Mat color={part.color} />
    </mesh>
  );
}

export function PropMesh({ parts, size }: { parts: PropPart[]; size: number }) {
  return (
    <group scale={size}>
      {parts.map((p, i) => (
        <PartMesh key={i} part={p} />
      ))}
    </group>
  );
}

/** Set up in front of the resident while it works. */
function Station({ work, accent }: { work: Work; accent: string }) {
  switch (work) {
    case "typing":
      return (
        <group>
          <mesh position-y={0.28}>
            <boxGeometry args={[0.6, 0.04, 0.32]} />
            <Mat color="#c08a5b" />
          </mesh>
          {[-1, 1].map((s) => (
            <mesh key={s} position={[s * 0.26, 0.13, 0]}>
              <boxGeometry args={[0.04, 0.26, 0.26]} />
              <Mat color="#a8754a" />
            </mesh>
          ))}
          <mesh position={[0, 0.31, -0.02]}>
            <boxGeometry args={[0.32, 0.02, 0.2]} />
            <Mat color="#d0d4da" />
          </mesh>
          <mesh position={[0, 0.41, 0.08]} rotation-x={0.35}>
            <boxGeometry args={[0.32, 0.2, 0.02]} />
            <Mat color="#d0d4da" />
          </mesh>
          <mesh position={[0, 0.41, 0.094]} rotation-x={0.35}>
            <planeGeometry args={[0.27, 0.16]} />
            <meshBasicMaterial color={accent} side={THREE.DoubleSide} />
          </mesh>
        </group>
      );
    case "hammering":
      return (
        <group position-y={0}>
          <mesh position-y={0.1}>
            <boxGeometry args={[0.18, 0.2, 0.18]} />
            <Mat color="#4a4f57" />
          </mesh>
          <mesh position-y={0.24}>
            <boxGeometry args={[0.38, 0.08, 0.2]} />
            <Mat color="#5c626b" />
          </mesh>
          <mesh position={[0.05, 0.31, 0]}>
            <boxGeometry args={[0.14, 0.05, 0.1]} />
            <Mat color="#ff9f43" emissive={0.7} />
          </mesh>
        </group>
      );
    case "stirring":
      return (
        <group>
          <mesh position-y={0.15}>
            <cylinderGeometry args={[0.24, 0.18, 0.3, 10]} />
            <Mat color="#2f3237" />
          </mesh>
          <mesh position-y={0.301} rotation-x={-Math.PI / 2}>
            <circleGeometry args={[0.21, 12]} />
            <meshBasicMaterial color={accent} />
          </mesh>
        </group>
      );
    case "reading":
    case "sweeping":
      return null;
  }
}

/** What the busy hand holds while working, if anything. */
function Tool({ work }: { work: Work }) {
  switch (work) {
    case "hammering":
      return (
        <group rotation-x={Math.PI / 2}>
          <mesh position-y={0.08}>
            <cylinderGeometry args={[0.02, 0.02, 0.26, 5]} />
            <Mat color="#9a6b47" />
          </mesh>
          <mesh position-y={0.22}>
            <boxGeometry args={[0.16, 0.07, 0.07]} />
            <Mat color="#7a7f88" />
          </mesh>
        </group>
      );
    case "stirring":
      return (
        <mesh position-y={-0.12}>
          <cylinderGeometry args={[0.018, 0.018, 0.4, 5]} />
          <Mat color="#c08a5b" />
        </mesh>
      );
    case "sweeping":
      return (
        <group position-y={-0.15} rotation-x={0.3}>
          <mesh>
            <cylinderGeometry args={[0.018, 0.018, 0.6, 5]} />
            <Mat color="#c08a5b" />
          </mesh>
          <mesh position-y={-0.33}>
            <coneGeometry args={[0.1, 0.16, 6]} />
            <Mat color="#e6c36a" />
          </mesh>
        </group>
      );
    case "typing":
    case "reading":
      return null;
  }
}

function darken(hex: string, by: number) {
  return `#${new THREE.Color(hex).multiplyScalar(by).getHexString()}`;
}

export interface CharacterHandlers {
  onSelect: (id: string) => void;
  onHover: (id: string | null) => void;
  onMenu: (id: string, x: number, y: number) => void;
}

/** Where a leash ties on, per resident, for `Leash` to read each frame. */
export type Anchors = Map<
  string,
  { hand?: THREE.Object3D; collar?: THREE.Object3D }
>;

function anchor(
  anchors: Anchors,
  id: string,
  kind: "hand" | "collar",
): (o: THREE.Object3D | null) => void {
  return (o) => {
    const entry = anchors.get(id) ?? {};
    if (o) entry[kind] = o;
    else delete entry[kind];
    anchors.set(id, entry);
  };
}

/** One resident, posed every frame from its agent in the sim. */
export function Character({
  resident,
  sim,
  handlers,
  anchors,
  collarColor,
}: {
  resident: Resident;
  sim: Sim;
  handlers: CharacterHandlers;
  anchors: Anchors;
  /** Its parent's colour, for a child; `null` wears no collar. */
  collarColor: string | null;
}) {
  const spec: AvatarSpec = resident.spec;
  const d = DIMS[spec.body];
  const root = useRef<THREE.Group>(null);
  const figure = useRef<THREE.Group>(null);
  const body = useRef<THREE.Group>(null);
  const eyes = useRef<THREE.Group>(null);
  const armL = useRef<THREE.Group>(null);
  const armR = useRef<THREE.Group>(null);
  const footL = useRef<THREE.Mesh>(null);
  const footR = useRef<THREE.Mesh>(null);
  const egg = useRef<THREE.Group>(null);
  const station = useRef<THREE.Group>(null);
  const tool = useRef<THREE.Group>(null);
  const book = useRef<THREE.Group>(null);
  const heldProp = useRef<THREE.Group>(null);
  const groundProp = useRef<THREE.Group>(null);
  const parcel = useRef<THREE.Group>(null);
  const ring = useRef<THREE.Mesh>(null);
  const alert = useRef<THREE.Mesh>(null);
  const propeller = useRef<THREE.Group>(null);

  const footColor = useMemo(
    () => darken(spec.body_color, 0.7),
    [spec.body_color],
  );
  const mood = resident.mood;
  const hat: Hat | "party" = mood === "shipped" ? "party" : spec.hat;

  useFrame((_, delta) => {
    const a: Agent | undefined = sim.agents.get(resident.id);
    if (!a || !root.current) return;
    const t = sim.time() + a.phase;
    const local = sim.time() - a.since;

    root.current.position.set(a.pos.x, 0, a.pos.z);
    root.current.rotation.y =
      a.facing +
      (a.activity === "portal_out" || a.activity === "portal_in"
        ? (1 - a.portalScale) * Math.PI * 3
        : 0);
    root.current.scale.setScalar(a.size * Math.max(0.001, a.portalScale));

    const isEgg = a.activity === "egg";
    egg.current!.visible = isEgg;
    figure.current!.visible = !isEgg;
    if (ring.current) {
      ring.current.visible = resident.selected;
      ring.current.scale.setScalar(1 + Math.sin(t * 3) * 0.05);
    }
    if (alert.current) {
      alert.current.visible = mood === "needs_you";
      alert.current.scale.setScalar(1 + Math.sin(t * 6) * 0.12);
    }
    if (propeller.current) propeller.current.rotation.y += delta * 14;

    if (isEgg) {
      const e = egg.current!;
      if (mood === "hatching") {
        const burst = Math.sin(t * 1.3) > 0.2 ? 1 : 0.15;
        e.rotation.z = Math.sin(t * 16) * 0.16 * burst;
      } else if (mood === "failed") {
        e.rotation.z = 0.4;
      } else {
        e.rotation.z = Math.sin(t * 2) * 0.04;
      }
      return;
    }

    const working = a.activity === "work";
    const shipping = mood === "shipping";
    station.current!.visible = working;
    tool.current!.visible =
      working &&
      (spec.work === "hammering" ||
        spec.work === "stirring" ||
        spec.work === "sweeping");
    book.current!.visible = working && spec.work === "reading";
    parcel.current!.visible = shipping && !working;
    const handsBusy = working || shipping;
    if (heldProp.current) heldProp.current.visible = !handsBusy;
    if (groundProp.current) groundProp.current.visible = working;

    const b = body.current!;
    const L = armL.current!;
    const R = armR.current!;
    b.position.y = 0;
    b.rotation.set(0, 0, 0);
    b.scale.set(1, 1, 1);
    L.rotation.set(0, 0, -0.12);
    R.rotation.set(0, 0, 0.12);
    footL.current!.position.set(-0.13, 0.05, 0.03);
    footR.current!.position.set(0.13, 0.05, 0.03);

    // Blink every few seconds; asleep keeps them shut.
    const blink = t % 4.2 < 0.12;
    eyes.current!.scale.y = a.activity === "sleep" ? 0.12 : blink ? 0.15 : 1;

    switch (a.activity) {
      case "walk": {
        const s = Math.sin(t * 11);
        b.position.y = Math.abs(s) * 0.06;
        b.rotation.z = s * 0.05;
        if (!handsBusy) {
          L.rotation.x = s * 0.6;
          R.rotation.x = -s * 0.6;
        }
        footL.current!.position.z = 0.03 + s * 0.09;
        footR.current!.position.z = 0.03 - s * 0.09;
        footL.current!.position.y = 0.05 + Math.max(0, s) * 0.05;
        footR.current!.position.y = 0.05 + Math.max(0, -s) * 0.05;
        break;
      }
      case "pause":
        b.scale.y = 1 + Math.sin(t * 2.2) * 0.02;
        b.rotation.y = Math.sin(t * 0.7) * 0.3;
        if (mood === "shipped" && Math.sin(t * 1.7) > 0.85) {
          b.position.y = Math.abs(Math.sin(t * 9)) * 0.12;
          L.rotation.z = -2.5;
          R.rotation.z = 2.5;
        }
        break;
      case "attention":
        b.position.y = Math.abs(Math.sin(t * 4.5)) * 0.1;
        R.rotation.z = 2.75 + Math.sin(t * 9) * 0.28;
        break;
      case "sleep":
        b.scale.y = 0.9 + Math.sin(t * 1.4) * 0.025;
        b.rotation.z = 0.18;
        break;
      case "work":
        poseWork(spec.work, t, local, b, L, R);
        break;
    }
    if (handsBusy && a.activity !== "work" && a.activity !== "attention") {
      L.rotation.x = -1.15;
      R.rotation.x = -1.15;
      L.rotation.z = 0.15;
      R.rotation.z = -0.15;
    }
  });

  const hit = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation();
    handlers.onSelect(resident.id);
  };

  return (
    <group
      ref={root}
      onClick={hit}
      onContextMenu={(e) => {
        e.stopPropagation();
        e.nativeEvent.preventDefault();
        handlers.onMenu(
          resident.id,
          e.nativeEvent.clientX,
          e.nativeEvent.clientY,
        );
      }}
      onPointerOver={(e) => {
        e.stopPropagation();
        handlers.onHover(resident.id);
      }}
      onPointerOut={() => handlers.onHover(null)}
    >
      {/* An easier target than the figure's own small parts. */}
      <mesh position-y={0.55}>
        <cylinderGeometry args={[0.45, 0.45, 1.2, 8]} />
        <meshBasicMaterial transparent opacity={0} depthWrite={false} />
      </mesh>
      <mesh rotation-x={-Math.PI / 2} position-y={0.01}>
        <circleGeometry args={[0.36, 18]} />
        <meshBasicMaterial
          color="#000000"
          transparent
          opacity={0.18}
          depthWrite={false}
        />
      </mesh>
      <mesh ref={ring} rotation-x={-Math.PI / 2} position-y={0.02}>
        <torusGeometry args={[0.55, 0.035, 6, 36]} />
        <meshBasicMaterial color="#ffffff" />
      </mesh>
      <mesh ref={alert} rotation-x={-Math.PI / 2} position-y={0.015}>
        <torusGeometry args={[0.46, 0.05, 6, 36]} />
        <meshBasicMaterial color="#ffd54f" />
      </mesh>

      <group ref={egg}>
        <mesh position-y={0.42} scale={[1, 1.3, 1]}>
          <sphereGeometry args={[0.32, 12, 10]} />
          <Mat color={mood === "failed" ? "#9e9e9e" : "#fff4dc"} />
        </mesh>
        <mesh position-y={0.42} rotation-x={Math.PI / 2}>
          <torusGeometry args={[0.315, 0.04, 6, 18]} />
          <Mat color={mood === "failed" ? "#6d6d6d" : spec.body_color} />
        </mesh>
      </group>

      <group ref={figure}>
        <group ref={body}>
          <BodyMesh body={spec.body} color={spec.body_color} />
          <group ref={eyes} position-y={d.eyeY}>
            <EyesMesh eyes={spec.eyes} d={d} accent={spec.accent_color} />
          </group>
          {[-1, 1].map((s) => (
            <mesh
              key={s}
              position={[s * (d.eyeX + 0.07), d.eyeY - 0.09, d.faceZ - 0.02]}
              scale={[1, 0.6, 0.4]}
            >
              <sphereGeometry args={[0.045, 8, 6]} />
              <meshBasicMaterial color="#ff8fa3" transparent opacity={0.55} />
            </mesh>
          ))}
          <HatMesh
            hat={hat}
            d={d}
            accent={spec.accent_color}
            propeller={propeller}
          />
          <AccessoryMesh
            accessory={spec.accessory}
            d={d}
            accent={spec.accent_color}
          />
          {collarColor && (
            <group position-y={d.neck}>
              <mesh rotation-x={Math.PI / 2}>
                <torusGeometry args={[d.hw * 0.96, 0.045, 6, 16]} />
                <Mat color={collarColor} />
              </mesh>
              <group
                ref={anchor(anchors, resident.id, "collar")}
                position={[0, -0.02, d.faceZ * 0.95]}
              />
            </group>
          )}
          <group ref={armL} position={[-(d.hw + 0.03), d.shoulder, 0]}>
            <mesh position-y={-0.15}>
              <capsuleGeometry args={[0.065, 0.18, 3, 6]} />
              <Mat color={spec.body_color} />
            </mesh>
            {spec.prop && (
              <group ref={heldProp} position={[-0.06, -0.34, 0.1]}>
                <PropMesh parts={spec.prop.parts} size={0.42} />
              </group>
            )}
          </group>
          <group ref={armR} position={[d.hw + 0.03, d.shoulder, 0]}>
            <mesh position-y={-0.15}>
              <capsuleGeometry args={[0.065, 0.18, 3, 6]} />
              <Mat color={spec.body_color} />
            </mesh>
            <group ref={tool} position-y={-0.3}>
              <Tool work={spec.work} />
            </group>
            <group
              ref={anchor(anchors, resident.id, "hand")}
              position-y={-0.3}
            />
          </group>
          <group
            ref={book}
            position={[0, d.shoulder - 0.22, d.faceZ + 0.2]}
            rotation-x={-0.5}
          >
            {[-1, 1].map((s) => (
              <mesh key={s} position-x={s * 0.1} rotation-y={s * -0.3}>
                <boxGeometry args={[0.2, 0.26, 0.02]} />
                <Mat color={s < 0 ? spec.accent_color : "#fdf6e3"} />
              </mesh>
            ))}
          </group>
          <group ref={parcel} position={[0, d.shoulder - 0.18, d.faceZ + 0.2]}>
            <mesh>
              <boxGeometry args={[0.34, 0.26, 0.26]} />
              <Mat color="#c9925b" />
            </mesh>
            <mesh>
              <boxGeometry args={[0.36, 0.05, 0.28]} />
              <Mat color={spec.accent_color} />
            </mesh>
          </group>
        </group>
        <mesh ref={footL}>
          <sphereGeometry args={[0.085, 8, 6]} />
          <Mat color={footColor} />
        </mesh>
        <mesh ref={footR}>
          <sphereGeometry args={[0.085, 8, 6]} />
          <Mat color={footColor} />
        </mesh>
        <group ref={station} position-z={0.5}>
          <Station work={spec.work} accent={spec.accent_color} />
        </group>
        {spec.prop && (
          <group ref={groundProp} position={[-0.62, 0.22, 0.2]}>
            <PropMesh parts={spec.prop.parts} size={0.42} />
          </group>
        )}
      </group>
    </group>
  );
}

function poseWork(
  work: Work,
  t: number,
  local: number,
  b: THREE.Group,
  L: THREE.Group,
  R: THREE.Group,
) {
  switch (work) {
    case "typing":
      L.rotation.x = -1.25 + Math.sin(t * 22) * 0.12;
      R.rotation.x = -1.25 + Math.sin(t * 22 + Math.PI) * 0.12;
      b.rotation.x = 0.06;
      break;
    case "hammering": {
      const s = (local * 2.2) % 1;
      R.rotation.x =
        s < 0.7 ? -0.9 - (s / 0.7) * 1.7 : -2.6 + ((s - 0.7) / 0.3) * 1.7;
      L.rotation.x = -0.5;
      b.rotation.x = s > 0.7 ? 0.08 : 0;
      break;
    }
    case "reading":
      L.rotation.x = -1.05;
      R.rotation.x = -1.05;
      L.rotation.z = 0.2;
      R.rotation.z = -0.2;
      b.rotation.z = Math.sin(t * 0.8) * 0.05;
      break;
    case "stirring":
      R.rotation.x = -1.0 + Math.sin(t * 4) * 0.2;
      R.rotation.z = 0.12 + Math.cos(t * 4) * 0.25;
      b.rotation.z = Math.sin(t * 4) * 0.04;
      break;
    case "sweeping":
      L.rotation.x = -0.75;
      R.rotation.x = -0.75;
      b.rotation.y = Math.sin(t * 3) * 0.4;
      break;
  }
}
