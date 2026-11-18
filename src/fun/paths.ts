/**
 * The way across a meadow round everything on it. Each meadow is baked into a
 * grid of walkable cells once per layout; a path is A* over the grid, pulled
 * tight to the corners it can't see past.
 */

import type { Layout, Obstacle, Region, Side, Vec2 } from "./sim";

const CELL = 0.2;
/** How far a walker's middle keeps off anything, the largest one's girth. */
const GIRTH = 0.21;
/** How far past its ground the grid runs, for spots on its edge. */
const BORDER = 1.6;
const OPEN = -1;
const FURNITURE = -2;

export interface Grid {
  x0: number;
  z0: number;
  w: number;
  h: number;
  /** Per cell: `OPEN`, `FURNITURE`, or the index of the block over it. */
  cells: Int16Array;
  blocks: Region[];
  obstacles: Obstacle[];
}

/** A way to `to`: the corners to pass, then `to` itself. */
export interface Path {
  points: Vec2[];
  /** The block `to` is inside, which is walked across to reach it. */
  through: number;
}

export function bake(layout: Layout, side: Side): Grid | null {
  const region = layout.regions[side];
  if (!region) return null;
  const x0 = region.xMin - BORDER;
  const z0 = region.zMin - BORDER;
  const w = Math.ceil((region.xMax - region.xMin + 2 * BORDER) / CELL);
  const h = Math.ceil((region.zMax - region.zMin + 2 * BORDER) / CELL);
  const cells = new Int16Array(w * h).fill(OPEN);
  /** Each cell whose middle is in `r` and passes `test` is set to `to`. */
  const stamp = (r: Region, to: number, test?: (p: Vec2) => boolean) => {
    const iMin = Math.max(0, Math.ceil((r.xMin - x0) / CELL - 0.5));
    const iMax = Math.min(w - 1, Math.floor((r.xMax - x0) / CELL - 0.5));
    const jMin = Math.max(0, Math.ceil((r.zMin - z0) / CELL - 0.5));
    const jMax = Math.min(h - 1, Math.floor((r.zMax - z0) / CELL - 0.5));
    for (let j = jMin; j <= jMax; j++) {
      for (let i = iMin; i <= iMax; i++) {
        if (
          !test ||
          test({ x: x0 + (i + 0.5) * CELL, z: z0 + (j + 0.5) * CELL })
        )
          cells[j * w + i] = to;
      }
    }
  };
  for (const o of layout.obstacles) {
    const r = o.r + GIRTH;
    stamp(
      grow({ xMin: o.x, xMax: o.x, zMin: o.z, zMax: o.z }, r),
      FURNITURE,
      (p) => dist(o, p) < r,
    );
  }
  // Backwards, so where two overlap the first is the one over it.
  for (let b = layout.blocks.length - 1; b >= 0; b--) {
    stamp(grow(layout.blocks[b], GIRTH), b);
  }
  // The pier is the way onto the water to the boat and the line along it.
  const lake = side === "left" ? layout.lake : null;
  if (lake) {
    stamp(
      {
        xMin: lake.pierEnd.x - 0.15,
        xMax: lake.pierFoot.x + 0.6,
        zMin: lake.pierEnd.z - 0.22,
        zMax: lake.pierEnd.z + 0.22,
      },
      OPEN,
    );
  }
  return {
    x0,
    z0,
    w,
    h,
    cells,
    blocks: layout.blocks,
    obstacles: layout.obstacles,
  };
}

export function walkable(g: Grid, p: Vec2, through = OPEN): boolean {
  if (!inGrid(g, p)) return false;
  const c = g.cells[index(g, p)];
  return c === OPEN || (through >= 0 && c === through);
}

/**
 * Whether the straight way from `a` to `b` is walkable all along: every cell
 * the line passes through, and both beside any corner it passes exactly
 * over. Exact rather than sampled, so a walker going along a line it could
 * see along never loses sight of the end of it.
 */
export function sees(g: Grid, a: Vec2, b: Vec2, through = OPEN): boolean {
  const open = (i: number, j: number) => {
    if (i < 0 || j < 0 || i >= g.w || j >= g.h) return false;
    const c = g.cells[j * g.w + i];
    return c === OPEN || (through >= 0 && c === through);
  };
  let i = Math.floor((a.x - g.x0) / CELL);
  let j = Math.floor((a.z - g.z0) / CELL);
  const iEnd = Math.floor((b.x - g.x0) / CELL);
  const jEnd = Math.floor((b.z - g.z0) / CELL);
  if (!open(i, j) || !open(iEnd, jEnd)) return false;
  const dx = b.x - a.x;
  const dz = b.z - a.z;
  const si = Math.sign(dx);
  const sj = Math.sign(dz);
  // How far along the line, 0 to 1, it next crosses a column and a row.
  const edge = (n: number, s: number, from: number, d: number, o: number) =>
    s === 0 ? Infinity : (o + (n + (s > 0 ? 1 : 0)) * CELL - from) / d;
  let tx = edge(i, si, a.x, dx, g.x0);
  let tz = edge(j, sj, a.z, dz, g.z0);
  const stepX = si === 0 ? Infinity : CELL / Math.abs(dx);
  const stepZ = sj === 0 ? Infinity : CELL / Math.abs(dz);
  for (let n = Math.abs(iEnd - i) + Math.abs(jEnd - j); n > 0; n--) {
    if (Math.abs(tx - tz) < 1e-9) {
      if (!open(i + si, j) || !open(i, j + sj)) return false;
      i += si;
      j += sj;
      tx += stepX;
      tz += stepZ;
      n--;
    } else if (tx < tz) {
      i += si;
      tx += stepX;
    } else {
      j += sj;
      tz += stepZ;
    }
    if (!open(i, j)) return false;
  }
  return true;
}

/**
 * The way from `from` to `to`. Either end off walkable ground, on a seat or
 * shoved against something, is reached by a straight step to or from the
 * nearest walkable cell; a goal inside a block, a ring on the pool, by
 * walking across the block. `null` when there's no way at all.
 */
export function findPath(g: Grid, from: Vec2, to: Vec2): Path | null {
  // Only a goal well inside, not one just off its edge, opens a block.
  const through = walkable(g, to)
    ? OPEN
    : g.blocks.findIndex((b) => inside(b, to));
  const s = nearestWalkable(g, from, through, to);
  const e = nearestWalkable(g, to, through, from);
  if (s === null || e === null) return null;
  const cells = astar(g, s, e, through);
  if (!cells) return null;
  // String-pulled: only the corners it can't see past.
  const points: Vec2[] = [];
  let anchor = cells[0];
  points.push(anchor);
  for (let k = 2; k < cells.length; k++) {
    if (!sees(g, anchor, cells[k], through)) {
      anchor = cells[k - 1];
      points.push(anchor);
    }
  }
  points.push(cells[cells.length - 1], { x: to.x, z: to.z });
  // Whatever it can already see past from where it stands is behind it.
  while (points.length > 1 && sees(g, from, points[1], through)) points.shift();
  return { points, through };
}

function astar(g: Grid, s: number, e: number, through: number): Vec2[] | null {
  const n = g.w * g.h;
  const cost = new Float32Array(n).fill(Infinity);
  const came = new Int32Array(n).fill(-1);
  const closed = new Uint8Array(n);
  const heap = new Heap();
  const ei = e % g.w;
  const ej = (e / g.w) | 0;
  const guess = (c: number) => {
    const dx = Math.abs((c % g.w) - ei);
    const dz = Math.abs(((c / g.w) | 0) - ej);
    return Math.max(dx, dz) + (Math.SQRT2 - 1) * Math.min(dx, dz);
  };
  const open = (i: number, j: number) => {
    if (i < 0 || j < 0 || i >= g.w || j >= g.h) return false;
    const c = g.cells[j * g.w + i];
    return c === OPEN || (through >= 0 && c === through);
  };
  cost[s] = 0;
  heap.push(s, guess(s));
  while (heap.size > 0) {
    const c = heap.pop();
    if (closed[c]) continue;
    closed[c] = 1;
    if (c === e) {
      const out: Vec2[] = [];
      for (let k = e; k !== -1; k = came[k]) out.push(centre(g, k));
      return out.reverse();
    }
    const i = c % g.w;
    const j = (c / g.w) | 0;
    for (let dj = -1; dj <= 1; dj++) {
      for (let di = -1; di <= 1; di++) {
        if ((!di && !dj) || !open(i + di, j + dj)) continue;
        // No cutting a corner between two cells that aren't both open.
        if (di && dj && (!open(i + di, j) || !open(i, j + dj))) continue;
        const m = (j + dj) * g.w + i + di;
        const next = cost[c] + (di && dj ? Math.SQRT2 : 1);
        if (next < cost[m]) {
          cost[m] = next;
          came[m] = c;
          heap.push(m, next + guess(m));
        }
      }
    }
  }
  return null;
}

/**
 * The walkable cell to step to from `p`, or `p`'s own if it's walkable. Of
 * those not much further than the nearest, the one on the side towards
 * `toward`, so someone getting up off a ring on the pool steps out on the
 * side it's going; and one it can step to straight, without crossing
 * anything but what it's in.
 */
function nearestWalkable(
  g: Grid,
  p: Vec2,
  through: number,
  toward: Vec2,
): number | null {
  const ci = clamp(Math.floor((p.x - g.x0) / CELL), 0, g.w - 1);
  const cj = clamp(Math.floor((p.z - g.z0) / CELL), 0, g.h - 1);
  if (walkable(g, p, through)) return cj * g.w + ci;
  let nearest: number | null = null;
  let nearestD = Infinity;
  let best: number | null = null;
  let bestScore = Infinity;
  for (let r = 0; r < Math.max(g.w, g.h); r++) {
    for (let j = cj - r; j <= cj + r; j++) {
      for (let i = ci - r; i <= ci + r; i++) {
        if (Math.max(Math.abs(i - ci), Math.abs(j - cj)) !== r) continue;
        if (i < 0 || j < 0 || i >= g.w || j >= g.h) continue;
        const c = g.cells[j * g.w + i];
        if (c !== OPEN && !(through >= 0 && c === through)) continue;
        const at = centre(g, j * g.w + i);
        const d = dist(at, p);
        if (d < nearestD) {
          nearestD = d;
          nearest = j * g.w + i;
        }
        const score = d + dist(at, toward);
        if (score < bestScore && clearStep(g, p, at)) {
          bestScore = score;
          best = j * g.w + i;
        }
      }
    }
    // Nothing on a further ring is near enough to be worth it.
    if (nearest !== null && (r - 1) * CELL > nearestD + 2) break;
  }
  return best ?? nearest;
}

/** Whether the straight step from `p` to `q` keeps out of everything but
 *  what `p` is in or up against. */
function clearStep(g: Grid, p: Vec2, q: Vec2): boolean {
  for (const o of g.obstacles) {
    if (dist(o, p) < o.r + GIRTH) continue;
    if (toSegment(o, p, q) < o.r) return false;
  }
  const n = Math.ceil(dist(p, q) / 0.05);
  for (const b of g.blocks) {
    if (inside(grow(b, GIRTH), p)) continue;
    for (let k = 1; k <= n; k++) {
      const t = k / n;
      if (inside(b, { x: p.x + (q.x - p.x) * t, z: p.z + (q.z - p.z) * t })) {
        return false;
      }
    }
  }
  return true;
}

/** How near the segment from `a` to `b` comes to `p`. */
function toSegment(p: Vec2, a: Vec2, b: Vec2): number {
  const dx = b.x - a.x;
  const dz = b.z - a.z;
  const l2 = dx * dx + dz * dz;
  const t = l2 ? clamp(((p.x - a.x) * dx + (p.z - a.z) * dz) / l2, 0, 1) : 0;
  return dist(p, { x: a.x + dx * t, z: a.z + dz * t });
}

class Heap {
  private items: number[] = [];
  private keys: number[] = [];
  get size() {
    return this.items.length;
  }
  push(item: number, key: number) {
    const { items, keys } = this;
    let c = items.length;
    items.push(item);
    keys.push(key);
    while (c > 0) {
      const p = (c - 1) >> 1;
      if (keys[p] <= key) break;
      items[c] = items[p];
      keys[c] = keys[p];
      c = p;
    }
    items[c] = item;
    keys[c] = key;
  }
  pop(): number {
    const { items, keys } = this;
    const top = items[0];
    const item = items.pop()!;
    const key = keys.pop()!;
    if (items.length === 0) return top;
    let c = 0;
    for (;;) {
      const l = 2 * c + 1;
      if (l >= items.length) break;
      const m = l + 1 < items.length && keys[l + 1] < keys[l] ? l + 1 : l;
      if (keys[m] >= key) break;
      items[c] = items[m];
      keys[c] = keys[m];
      c = m;
    }
    items[c] = item;
    keys[c] = key;
    return top;
  }
}

function index(g: Grid, p: Vec2) {
  return (
    Math.floor((p.z - g.z0) / CELL) * g.w + Math.floor((p.x - g.x0) / CELL)
  );
}

function inGrid(g: Grid, p: Vec2) {
  const i = Math.floor((p.x - g.x0) / CELL);
  const j = Math.floor((p.z - g.z0) / CELL);
  return i >= 0 && j >= 0 && i < g.w && j < g.h;
}

function centre(g: Grid, c: number): Vec2 {
  return {
    x: g.x0 + ((c % g.w) + 0.5) * CELL,
    z: g.z0 + (((c / g.w) | 0) + 0.5) * CELL,
  };
}

function grow(r: Region, by: number): Region {
  return {
    xMin: r.xMin - by,
    xMax: r.xMax + by,
    zMin: r.zMin - by,
    zMax: r.zMax + by,
  };
}

function inside(r: Region, p: Vec2) {
  return p.x >= r.xMin && p.x <= r.xMax && p.z >= r.zMin && p.z <= r.zMax;
}

const dist = (a: Vec2, b: Vec2) => Math.hypot(a.x - b.x, a.z - b.z);
const clamp = (n: number, min: number, max: number) =>
  Math.min(max, Math.max(min, n));
