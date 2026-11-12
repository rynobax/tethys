/**
 * An avatar is a description, never a picture, so any renderer can draw it in
 * its own style. The vocabulary is `SPEC_SCHEMA` in `src-tauri/src/avatar.rs`;
 * everything here is re-checked anyway, since a bad field must degrade to a
 * default rather than break the scene.
 */

export const BODIES = ["bean", "blob", "cube", "tall", "round"] as const;
export const EYES = ["dots", "wide", "sleepy", "visor", "cyclops"] as const;
export const HATS = [
  "none",
  "cap",
  "beanie",
  "wizard",
  "hardhat",
  "crown",
  "antenna",
  "chef",
  "headphones",
  "propeller",
] as const;
export const ACCESSORIES = [
  "none",
  "scarf",
  "glasses",
  "cape",
  "backpack",
  "bowtie",
] as const;
export const WORKS = [
  "typing",
  "hammering",
  "reading",
  "stirring",
  "sweeping",
] as const;
export const SHAPES = ["box", "sphere", "cylinder", "cone", "torus"] as const;

export type Body = (typeof BODIES)[number];
export type Eyes = (typeof EYES)[number];
export type Hat = (typeof HATS)[number];
export type Accessory = (typeof ACCESSORIES)[number];
export type Work = (typeof WORKS)[number];
export type Shape = (typeof SHAPES)[number];

export type Vec3 = [number, number, number];

export interface PropPart {
  shape: Shape;
  color: string;
  position: Vec3;
  scale: Vec3;
  /** Degrees. */
  rotation: Vec3;
}

export interface AvatarSpec {
  name: string;
  tagline: string;
  body: Body;
  body_color: string;
  accent_color: string;
  eyes: Eyes;
  hat: Hat;
  accessory: Accessory;
  work: Work;
  prop: { label: string; parts: PropPart[] } | null;
}

function pick<T extends string>(
  options: readonly T[],
  value: unknown,
  fallback: T,
): T {
  return options.includes(value as T) ? (value as T) : fallback;
}

const HEX = /^#[0-9a-fA-F]{6}$/;
const color = (value: unknown, fallback: string) =>
  typeof value === "string" && HEX.test(value) ? value : fallback;

const text = (value: unknown, max: number) =>
  typeof value === "string" ? value.slice(0, max) : "";

function vec3(value: unknown, fallback: Vec3, min: number, max: number): Vec3 {
  if (!Array.isArray(value) || value.length !== 3) return fallback;
  return value.map((n, i) =>
    typeof n === "number" && Number.isFinite(n)
      ? Math.min(max, Math.max(min, n))
      : fallback[i],
  ) as Vec3;
}

function part(raw: unknown): PropPart | null {
  if (!raw || typeof raw !== "object") return null;
  const p = raw as Record<string, unknown>;
  return {
    shape: pick(SHAPES, p.shape, "box"),
    color: color(p.color, "#cccccc"),
    position: vec3(p.position, [0, 0, 0], -0.5, 0.5),
    scale: vec3(p.scale, [0.4, 0.4, 0.4], 0.04, 1),
    rotation: vec3(p.rotation, [0, 0, 0], -360, 360),
  };
}

export function normalizeSpec(raw: unknown, fallback: AvatarSpec): AvatarSpec {
  if (!raw || typeof raw !== "object") return fallback;
  const r = raw as Record<string, unknown>;
  const rawProp = r.prop as Record<string, unknown> | undefined;
  const parts = Array.isArray(rawProp?.parts)
    ? rawProp.parts.slice(0, 8).flatMap((p) => part(p) ?? [])
    : [];
  return {
    name: text(r.name, 24) || fallback.name,
    tagline: text(r.tagline, 70),
    body: pick(BODIES, r.body, fallback.body),
    body_color: color(r.body_color, fallback.body_color),
    accent_color: color(r.accent_color, fallback.accent_color),
    eyes: pick(EYES, r.eyes, fallback.eyes),
    hat: pick(HATS, r.hat, "none"),
    accessory: pick(ACCESSORIES, r.accessory, "none"),
    work: pick(WORKS, r.work, fallback.work),
    prop: parts.length
      ? { label: text(rawProp?.label, 40), parts }
      : null,
  };
}

/** Stable per id, so a workspace keeps its look until it's designed. */
export function hashString(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

const DEFAULT_COLORS = [
  ["#f4a6b8", "#7c4dff"],
  ["#9ad0f5", "#ff8a65"],
  ["#b9e4a8", "#5c6bc0"],
  ["#ffd580", "#26a69a"],
  ["#d7b8f3", "#ffb300"],
  ["#ffc4a3", "#42a5f5"],
] as const;

/** The plain bean a workspace is until its first turn says what it's about. */
export function defaultSpec(id: string, branch: string): AvatarSpec {
  const h = hashString(id);
  const [body, accent] = DEFAULT_COLORS[h % DEFAULT_COLORS.length];
  return {
    name: branch,
    tagline: "",
    body: "bean",
    body_color: body,
    accent_color: accent,
    eyes: "dots",
    hat: "none",
    accessory: "none",
    work: WORKS[(h >>> 4) % WORKS.length],
    prop: null,
  };
}
