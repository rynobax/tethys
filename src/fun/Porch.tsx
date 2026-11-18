import { useMemo, useState } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { ANIMALS } from "./animals";
import { Mat } from "./animals/rig";
import type { Porch, QuiltingPlace, Sim } from "./sim";
import { useEnvironment, type Environment } from "./environment";
import { DeskChair } from "./environments/office";

const WOOD = "#9a6b47";
const DARK_WOOD = "#7a5236";
const DECK = "#c49a6c";
/** Bare cloth, and the squares on it still to be sewn. */
const MUSLIN = "#f3ead6";
const UNSEWN = "#e2d5b8";

/** The quilt on its frame, across and from the quilter to the viewer. */
const QUILT_W = 1.15;
const QUILT_D = 0.72;
/** How high the quilt is stretched, at a sitter's lap. */
const QUILT_Y = 0.31;

/** The porch's boards, alternating; a rug in the office. */
const DECKS: Record<Environment, [string, string]> = {
  meadow: [DECK, "#b88d60"],
  moon: ["#b4bac4", "#a8aeb9"],
  athens: ["#efe9dc", "#e4dccb"],
  office: ["#c96f5a", "#bd6450"],
};

/** A rocking chair facing the viewer, its rockers along z. */
function RockingChair({
  wood = WOOD,
  rocker = DARK_WOOD,
}: {
  wood?: string;
  rocker?: string;
}) {
  return (
    <group>
      <mesh position-y={0.2}>
        <boxGeometry args={[0.42, 0.04, 0.38]} />
        <Mat color={wood} />
      </mesh>
      {[-0.17, 0.17].map((x) => (
        <group key={x} position-x={x}>
          {[-0.13, 0.13].map((z) => (
            <mesh key={z} position={[0, 0.11, z]}>
              <boxGeometry args={[0.04, 0.16, 0.04]} />
              <Mat color={wood} />
            </mesh>
          ))}
          {/* The rocker: a shallow arc under both legs. */}
          <group position-y={0.62} rotation-y={Math.PI / 2}>
            <mesh rotation-z={-Math.PI / 2 - 0.375}>
              <torusGeometry args={[0.6, 0.018, 3, 10, 0.75]} />
              <Mat color={rocker} />
            </mesh>
          </group>
          <mesh position={[0, 0.48, -0.19]}>
            <boxGeometry args={[0.04, 0.6, 0.04]} />
            <Mat color={wood} />
          </mesh>
        </group>
      ))}
      {[0.36, 0.56, 0.74].map((y) => (
        <mesh key={y} position={[0, y, -0.19]}>
          <boxGeometry args={[0.38, 0.07, 0.03]} />
          <Mat color={wood} />
        </mesh>
      ))}
    </group>
  );
}

/** One quilter's quilt: how it's sewn, read off the sim. */
interface Sewing {
  done: number;
  total: number;
  /** The family's colours, for the squares sewn. */
  colors: string[];
}

function sewingOf(sim: Sim, index: number): Sewing | null {
  const r = sim.quilter(index);
  if (!r) return null;
  const { accent, coat } = ANIMALS[r.animal];
  const light = `#${new THREE.Color(accent).lerp(new THREE.Color("#ffffff"), 0.45).getHexString()}`;
  // A coat as pale as the cloth would vanish on it: a deeper accent instead.
  const pale = new THREE.Color(coat).getHSL({ h: 0, s: 0, l: 0 }).l > 0.8;
  const third = pale
    ? `#${new THREE.Color(accent).multiplyScalar(0.7).getHexString()}`
    : coat;
  return {
    done: r.progress?.done ?? 0,
    total: r.progress?.total ?? 0,
    colors: [accent, third, light],
  };
}

const sameSewing = (a: Sewing | null, b: Sewing | null) =>
  a === b ||
  (!!a &&
    !!b &&
    a.done === b.done &&
    a.total === b.total &&
    a.colors[0] === b.colors[0]);

/**
 * A square for each task, sewn in the family's colours as each is done and
 * bare cloth till then, filled from the quilter's side. Bound in its accent
 * once every square is in. Plain cloth with no tasks to count, or nobody at it.
 */
function Quilt({ index, sim }: { index: number; sim: Sim }) {
  const [sewing, setSewing] = useState<Sewing | null>(null);
  useFrame(() => {
    const next = sewingOf(sim, index);
    if (!sameSewing(next, sewing)) setSewing(next);
  });
  const squares = useMemo(() => {
    if (!sewing || sewing.total <= 0) return [];
    const { total, done, colors } = sewing;
    const cols = Math.ceil(Math.sqrt((total * QUILT_W) / QUILT_D));
    const rows = Math.ceil(total / cols);
    const cell = Math.min((QUILT_W - 0.04) / cols, (QUILT_D - 0.04) / rows);
    return Array.from({ length: total }, (_, i) => {
      const c = i % cols;
      const r = Math.floor(i / cols);
      // The last row centred, when it's short.
      const inRow = r === rows - 1 ? total - r * cols : cols;
      return {
        x: (c - (inRow - 1) / 2) * cell,
        z: (r - (rows - 1) / 2) * cell,
        size: cell * 0.9,
        color: i < done ? colors[(r + c) % colors.length] : UNSEWN,
      };
    });
  }, [sewing]);
  const finished = !!sewing && sewing.total > 0 && sewing.done >= sewing.total;
  return (
    <group position-y={QUILT_Y}>
      {finished && (
        <mesh position-y={-0.004} rotation-x={-Math.PI / 2}>
          <planeGeometry args={[QUILT_W + 0.08, QUILT_D + 0.08]} />
          <meshStandardMaterial color={sewing.colors[0]} flatShading />
        </mesh>
      )}
      <mesh rotation-x={-Math.PI / 2}>
        <planeGeometry args={[QUILT_W, QUILT_D]} />
        <meshStandardMaterial color={MUSLIN} flatShading />
      </mesh>
      {squares.map((q, i) => (
        <mesh key={i} position={[q.x, 0.004, q.z]} rotation-x={-Math.PI / 2}>
          <planeGeometry args={[q.size, q.size]} />
          <meshStandardMaterial color={q.color} flatShading />
        </mesh>
      ))}
    </group>
  );
}

/** A low frame holding the quilt out flat, rails along each long side. */
function QuiltingFrame({ index, sim }: { index: number; sim: Sim }) {
  const legX = QUILT_W / 2 + 0.04;
  const railZ = QUILT_D / 2 + 0.03;
  return (
    <group>
      {[-railZ, railZ].map((z) => (
        <mesh key={z} position={[0, QUILT_Y, z]} rotation-z={Math.PI / 2}>
          <cylinderGeometry args={[0.025, 0.025, QUILT_W + 0.2, 6]} />
          <Mat color={WOOD} />
        </mesh>
      ))}
      {[-legX, legX].flatMap((x) =>
        [-railZ, railZ].map((z) => (
          <mesh key={`${x}${z}`} position={[x, QUILT_Y / 2, z]}>
            <boxGeometry args={[0.04, QUILT_Y, 0.04]} />
            <Mat color={DARK_WOOD} />
          </mesh>
        )),
      )}
      <Quilt index={index} sim={sim} />
    </group>
  );
}

function Chair() {
  const environment = useEnvironment();
  if (environment === "office") return <DeskChair />;
  if (environment === "moon")
    return <RockingChair wood="#d3d7df" rocker="#7d838f" />;
  return <RockingChair />;
}

function Place({
  place,
  index,
  sim,
}: {
  place: QuiltingPlace;
  index: number;
  sim: Sim;
}) {
  return (
    <>
      <group position={[place.seat.x, 0.03, place.seat.z]}>
        <Chair />
      </group>
      <group position={[place.frame.x, 0.03, place.frame.z]}>
        <QuiltingFrame index={index} sim={sim} />
      </group>
    </>
  );
}

/** Boards under the row of places, and a chair and a frame at each. */
export function PorchArea({ porch, sim }: { porch: Porch; sim: Sim }) {
  const [boards, between] = DECKS[useEnvironment()];
  const deck = useMemo(() => {
    const xs = porch.places.map((p) => p.seat.x);
    const { seat, frame } = porch.places[0];
    return {
      xMin: Math.min(...xs) - 1,
      xMax: Math.max(...xs) + 1,
      zMin: seat.z - 0.55,
      zMax: frame.z + 0.55,
    };
  }, [porch]);
  const planks = Math.round((deck.zMax - deck.zMin) / 0.22);
  return (
    <>
      {Array.from({ length: planks }, (_, i) => (
        <mesh
          key={i}
          position={[
            (deck.xMin + deck.xMax) / 2,
            0.015,
            deck.zMin + (i + 0.5) * ((deck.zMax - deck.zMin) / planks),
          ]}
        >
          <boxGeometry
            args={[
              deck.xMax - deck.xMin,
              0.03,
              (deck.zMax - deck.zMin) / planks - 0.015,
            ]}
          />
          <Mat color={i % 2 ? boards : between} />
        </mesh>
      ))}
      {porch.places.map((p, i) => (
        <Place key={i} place={p} index={i} sim={sim} />
      ))}
    </>
  );
}
