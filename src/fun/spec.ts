/** What a resident does at a spot of its own while working. */
export const WORKS = [
  "typing",
  "hammering",
  "reading",
  "stirring",
  "sweeping",
] as const;

export type Work = (typeof WORKS)[number];

/** Stable per id, for anything picked once and kept. */
export function hashString(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

export const workFor = (id: string): Work =>
  WORKS[(hashString(id) >>> 4) % WORKS.length];
