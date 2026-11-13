import type { ComponentType, RefObject } from "react";
import * as THREE from "three";
import { mergeGeometries } from "three/examples/jsm/utils/BufferGeometryUtils.js";

/**
 * The skeleton every animal shares, in the units of a top-level resident
 * (`sizeForDepth` scales the whole figure). Every pose, and everything held,
 * carried or set down, is placed against these, so an animal draws its parts
 * at these joints rather than moving them.
 */
export const RIG = {
  /** Each foot's centre, a little forward; walking swings them from here. */
  foot: { x: 0.13, y: 0.05, z: 0.03 },
  /** The torso's bottom and top, and how far its front comes forward. */
  hip: 0.14,
  chest: 0.6,
  front: 0.22,
  /** Each arm hangs from here; the hand is `hand` below it. */
  shoulder: { x: 0.25, y: 0.55 },
  hand: 0.3,
  neck: 0.62,
  /** The head's centre, where `Head` is drawn from. */
  head: 0.88,
  /** Where a tail joins, at the small of the back. */
  tail: { y: 0.24, z: -0.18 },
} as const;

/** A child's head is drawn this much bigger, for baby proportions. */
export const YOUNG_HEAD = 1.18;

/** Which side a limb is on: -1 left, 1 right (the figure's own). */
export type Side = -1 | 1;

export interface AnimalDesign {
  /** Its fur or skin, where something must match it (the egg's band). */
  coat: string;
  /** Its own colour, for what it uses: the screen, a book, its drink. */
  accent: string;
  /** Its highest point, ears included, for placing its name tag. */
  top: number;
  /**
   * Drawn from `RIG.head`. Eyes go in a group given `eyes` as its ref, at eye
   * height: blinking and wonder scale that group about its origin.
   */
  Head: ComponentType<{ eyes: RefObject<THREE.Group | null> }>;
  /** Hips to shoulders, drawn from the ground. */
  Torso: ComponentType;
  /** Drawn from the shoulder, hanging down to the hand at `-RIG.hand`. */
  Arm: ComponentType<{ side: Side }>;
  /** Drawn from the foot's centre. */
  Foot: ComponentType<{ side: Side }>;
  /** Drawn from `RIG.tail`; it sways from there. */
  Tail?: ComponentType;
}

/** The low-poly look the whole world uses. */
export function Mat({ color, emissive }: { color: string; emissive?: number }) {
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

/** Two round eyes with a glint, at eye height in the eyes group. */
export function DotEyes({
  eyes,
  y,
  x,
  z,
  r = 0.045,
  color = "#1d1d1f",
}: {
  eyes: RefObject<THREE.Group | null>;
  y: number;
  /** Each eye's distance from the middle. */
  x: number;
  /** How far forward, on the face's surface. */
  z: number;
  r?: number;
  color?: string;
}) {
  return (
    <group ref={eyes} position-y={y}>
      {[-1, 1].map((s) => (
        <group key={s} position={[s * x, 0, z]}>
          <mesh scale={[1, 1.15, 0.6]}>
            <sphereGeometry args={[r, 8, 6]} />
            <Mat color={color} />
          </mesh>
          <mesh position={[r * 0.35, r * 0.4, r * 0.5]}>
            <sphereGeometry args={[r * 0.3, 5, 4]} />
            <meshBasicMaterial color="#ffffff" />
          </mesh>
        </group>
      ))}
    </group>
  );
}

/** Rosy cheeks, below and outside the eyes. */
export function Blush({ y, x, z }: { y: number; x: number; z: number }) {
  return (
    <>
      {[-1, 1].map((s) => (
        <mesh key={s} position={[s * x, y, z]} scale={[1, 0.6, 0.4]}>
          <sphereGeometry args={[0.045, 8, 6]} />
          <meshBasicMaterial color="#ff8fa3" transparent opacity={0.55} />
        </mesh>
      ))}
    </>
  );
}

/** A plain arm in one colour, its hand a little rounder. */
export function SimpleArm({ color, hand }: { color: string; hand?: string }) {
  return (
    <>
      <mesh position-y={-0.14}>
        <capsuleGeometry args={[0.06, 0.16, 3, 6]} />
        <Mat color={color} />
      </mesh>
      <mesh position-y={-RIG.hand + 0.03}>
        <sphereGeometry args={[0.068, 7, 5]} />
        <Mat color={hand ?? color} />
      </mesh>
    </>
  );
}

/** A plain foot, longer than it's wide. */
export function SimpleFoot({ color }: { color: string }) {
  return (
    <mesh scale={[1, 0.75, 1.25]}>
      <sphereGeometry args={[0.085, 8, 6]} />
      <Mat color={color} />
    </mesh>
  );
}

/**
 * A stripe lying on a sphere, `yaw` round from the front. One that goes `up`
 * runs between two angles above the equator; any other runs round the sphere
 * at height `y`, `span` radians long.
 */
export type Stripe =
  | { up: true; yaw: number; from: number; to: number; w?: number }
  | { up?: false; y: number; yaw: number; span: number; w?: number };

/** Stripes on a sphere of radius `r`, sunk a little into it, in one mesh. */
export function stripes(r: number, list: Stripe[]): THREE.BufferGeometry {
  const parts = list.map((s) => {
    const w = s.w ?? 0.022;
    const sunk = r - w * 0.4;
    if (s.up) {
      return new THREE.TorusGeometry(sunk, w, 3, 5, s.to - s.from)
        .rotateZ(s.from)
        .rotateY(s.yaw - Math.PI / 2);
    }
    return new THREE.TorusGeometry(
      Math.sqrt(sunk * sunk - s.y * s.y),
      w,
      3,
      Math.max(3, Math.round(s.span * 5)),
      s.span,
    )
      .rotateZ(Math.PI / 2 - s.yaw - s.span / 2)
      .rotateX(Math.PI / 2)
      .translate(0, s.y, 0);
  });
  const merged = mergeGeometries(parts)!;
  for (const p of parts) p.dispose();
  return merged;
}
