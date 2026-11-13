/**
 * The camera is fixed and orthographic, looking straight ahead and down, so
 * screen x maps linearly to world x and screen y to world z. That's what lets
 * the meadows be measured off the terminal panel's own rectangle.
 */

import type {
  Lake,
  Layout,
  Lounge,
  Obstacle,
  Pitch,
  Region,
  Seat,
  Vec2,
  Workshop,
} from "./sim";

/** Pixels per world unit; a top-level resident is about one unit tall. */
export const PIXELS_PER_UNIT = 54;
export const PITCH = (40 * Math.PI) / 180;
export const CAMERA_DISTANCE = 40;

/** A little ground kept clear at the window's edge and beside the panel. Name
 *  tags needn't fit inside it: they slide to stay on screen and clear of the
 *  panel (`LabelProjector`). */
const EDGE_MARGIN = 0.6;
const PANEL_MARGIN = 0.6;
/** The treeline and portal live in this strip at the back. */
const BACK_STRIP = 2.4;
const FRONT_MARGIN = 0.6;
const MIN_WIDTH = 1.4;

/** A waiting resident's footprint on screen: a name tag wide, a figure tall. */
const SLOT_WIDTH_PX = 200;
const SLOT_HEIGHT_PX = 136;

/** How high the pitch stage's boards stand, for whoever is on them. */
export const STAGE_HEIGHT = 0.2;
/** A seated resident's footprint: a compact name tag wide, a short figure tall. */
const CHAIR_WIDTH_PX = 140;
const CHAIR_ROW_PX = 76;
const CHAIR_ROWS = 3;
/** How far a portal sits in from its meadow's edge by the panel. */
const PORTAL_INSET = 1;
/** Below this the left meadow is too narrow to give a corner to a lake. */
const MIN_LAKE_WIDTH = 4;

/** Below this a meadow is too short to split between two uses. */
const MIN_ZONED_DEPTH = 12;

export interface Bounds {
  xMin: number;
  xMax: number;
  zMin: number;
  zMax: number;
}

export interface Stage {
  layout: Layout;
  /** All the ground in view. */
  visible: Bounds;
  /** The ground the terminal panel covers, in x. */
  panel: { xMin: number; xMax: number };
}

export function computeStage(
  width: number,
  height: number,
  panel: { left: number; right: number },
): Stage {
  const toX = (px: number) => (px - width / 2) / PIXELS_PER_UNIT;
  const toZ = (py: number) =>
    (py - height / 2) / (PIXELS_PER_UNIT * Math.sin(PITCH));
  const visible = {
    xMin: toX(0),
    xMax: toX(width),
    zMin: toZ(0),
    zMax: toZ(height),
  };
  const panelX = { xMin: toX(panel.left), xMax: toX(panel.right) };

  const region = (xMin: number, xMax: number): Region | null =>
    xMax - xMin < MIN_WIDTH
      ? null
      : {
          xMin,
          xMax,
          zMin: visible.zMin + BACK_STRIP,
          zMax: visible.zMax - FRONT_MARGIN,
        };
  const left = region(visible.xMin + EDGE_MARGIN, panelX.xMin - PANEL_MARGIN);
  const right = region(panelX.xMax + PANEL_MARGIN, visible.xMax - EDGE_MARGIN);
  // Each portal in its meadow's back corner by the panel, so the two face
  // each other across it.
  const portal = (r: Region | null, side: "left" | "right") =>
    r
      ? {
          x: side === "left" ? r.xMax - PORTAL_INSET : r.xMin + PORTAL_INSET,
          z: r.zMin - 0.2,
        }
      : null;

  // The left meadow's bottom is the stage, its back corner away from the
  // panel the lake; waiting residents keep to the rest.
  const stage = left ? pitch(left) : null;
  const pond = left ? lake(left, visible) : null;
  const waiting = left && stage ? { ...left, zMax: stage.top } : left;
  const zones = rightZones(right);
  const shore = pond && grow(pond.shore, 0.4);
  return {
    layout: {
      regions: { left, right },
      portals: { left: portal(left, "left"), right: portal(right, "right") },
      attentionSlots: waiting
        ? attentionSlots(waiting).filter((p) => !shore || !within(shore, p))
        : [],
      pitch: stage,
      lake: pond,
      ...zones,
      obstacles: obstacles(zones),
      // Walked round, unless heading for a ring on the pool or the boat.
      blocks: [zones.lounge?.pool, pond?.water].filter((b) => !!b),
    },
    visible,
    panel: panelX,
  };
}

/**
 * A staggered grid over the left meadow, filled from its middle outward, so a
 * few waiting residents gather in a loose cluster rather than a queue.
 */
function attentionSlots(region: Region): Vec2[] {
  const colGap = SLOT_WIDTH_PX / PIXELS_PER_UNIT;
  const rowGap = SLOT_HEIGHT_PX / (PIXELS_PER_UNIT * Math.sin(PITCH));
  const halfTag = colGap / 2;
  const top = region.zMin + rowGap * 0.6;
  const middle = (region.zMin + region.zMax) / 2;
  // In from the panel's edge by the rest of half a name tag.
  const inner = region.xMax - Math.max(0, halfTag - PANEL_MARGIN) - 0.15;
  const slots: Vec2[] = [];
  for (let c = 0; ; c++) {
    const x = inner - c * colGap;
    if (x < region.xMin - 0.1) break;
    const offset = c % 2 ? rowGap / 2 : 0;
    for (let z = top + offset; z <= region.zMax; z += rowGap) {
      slots.push({ x, z });
    }
  }
  const centre = (Math.max(region.xMin, inner - colGap) + inner) / 2;
  const pxFrom = (p: Vec2) =>
    Math.hypot(
      (centre - p.x) * PIXELS_PER_UNIT,
      (p.z - middle) * PIXELS_PER_UNIT * Math.sin(PITCH),
    );
  return slots.sort((a, b) => pxFrom(a) - pxFrom(b));
}

/**
 * The bottom of the left meadow: a stage where one Shipping resident pitches
 * at a time from the front, the rest waiting in rows of chairs behind it, and
 * a crowd in front with its back to the viewer.
 */
function pitch(region: Region): Pitch | null {
  if (region.zMax - region.zMin < MIN_ZONED_DEPTH) return null;
  const sinPitch = Math.sin(PITCH);
  const colGap = CHAIR_WIDTH_PX / PIXELS_PER_UNIT;
  const rowGap = CHAIR_ROW_PX / (PIXELS_PER_UNIT * sinPitch);
  const front = region.zMax - 1.9;
  const width = region.xMax - region.xMin;
  const centre = (region.xMin + region.xMax) / 2;
  const middleOut = (a: Vec2, b: Vec2) =>
    Math.abs(a.x - centre) - Math.abs(b.x - centre);

  // Rows of as many tag-widths as fit across, centred, the first right behind
  // the podium and filled before the next behind it, each offset half a chair
  // from the one in front so no tag sits straight above another.
  const n = Math.max(1, Math.floor(width / colGap) + 1);
  const chairs: Vec2[] = [];
  const chairRows: number[] = [];
  for (let r = 0; r < CHAIR_ROWS; r++) {
    const z = front - (r + 1) * rowGap;
    const count = r % 2 ? Math.max(1, n - 1) : n;
    const row = Array.from({ length: count }, (_, i) => ({
      x: centre + (i - (count - 1) / 2) * colGap,
      z,
    }));
    chairs.push(...row.sort(middleOut));
    chairRows.push(...row.map(() => r));
  }
  // The stage for each number of rows in use, growing back from the podium.
  const stages = Array.from({ length: CHAIR_ROWS }, (_, r) => ({
    xMin: region.xMin - 0.6,
    xMax: region.xMax + 0.6,
    zMin: front - (r + 1) * rowGap - 0.7,
    zMax: front + 0.6,
  }));
  const stage = stages[CHAIR_ROWS - 1];
  const crowd: Vec2[] = [];
  for (const [z, shift] of [
    [region.zMax + 0.2, 0],
    [region.zMax - 0.45, 0.3],
  ] as const) {
    for (let x = stage.xMin + 0.3 + shift; x < stage.xMax - 0.2; x += 0.6) {
      // Deterministic jitter, so a resize doesn't reshuffle the crowd much.
      const j = Math.sin(x * 12.9898 + z * 78.233) * 43758.5453;
      crowd.push({ x: x + (j - Math.floor(j) - 0.5) * 0.2, z });
    }
  }
  return {
    podium: { x: centre, z: front },
    chairs,
    chairRows,
    crowd,
    stages,
    stage,
    top: stage.zMin - 0.4,
  };
}

/** The right meadow: the lounge across its back half, the workshop its front. */
function rightZones(
  region: Region | null,
): Pick<Layout, "lounge" | "workshop"> {
  if (!region || region.zMax - region.zMin < MIN_ZONED_DEPTH) {
    return { lounge: null, workshop: null };
  }
  const half = (region.zMin + region.zMax) / 2;
  return {
    lounge: lounge({ ...region, zMax: half - 0.3 }),
    workshop: workshop({ ...region, zMin: half + 0.3 }),
  };
}

/** What a walker steps round: the lounge's and the workshop's furniture. */
function obstacles({
  lounge,
  workshop,
}: Pick<Layout, "lounge" | "workshop">): Obstacle[] {
  const out: Obstacle[] = [];
  if (lounge) {
    // One circle over a lounger and its table together, which leaves no
    // notch between the two for a walker to wedge itself into.
    const pool = (lounge.pool.xMin + lounge.pool.xMax) / 2;
    for (const p of lounge.seats) {
      const across = p.x < pool ? -0.12 : 0.12;
      out.push({ ...onSeat(p, across, -0.25), r: 0.5 });
    }
    for (const p of lounge.parasols) out.push({ ...p, r: 0.15 });
  }
  if (workshop) {
    // The board stands behind its writer's spot, a few circles wide.
    for (const p of workshop.boards) {
      for (const dx of [-0.3, 0, 0.3]) {
        out.push({ x: p.x + dx, z: p.z - 0.5, r: 0.2 });
      }
    }
    for (const p of workshop.piles) out.push({ ...p, r: 0.3 });
  }
  return out;
}

/**
 * The back corner of the left meadow away from the panel: water running off
 * the window's edge, a pier out into it from the shore, and the shore round
 * it where the merged wait to be deleted.
 */
function lake(region: Region, visible: Bounds): Lake | null {
  const width = region.xMax - region.xMin;
  if (width < MIN_LAKE_WIDTH) return null;
  const water = {
    xMin: visible.xMin - 2,
    xMax: region.xMin + Math.min(4, width * 0.42),
    zMin: region.zMin - 0.8,
    zMax: region.zMin + 3.4,
  };
  const z = (water.zMin + water.zMax) / 2 + 0.5;
  return {
    water,
    shore: {
      xMin: region.xMin,
      xMax: Math.min(region.xMax, water.xMax + 2.6),
      zMin: region.zMin,
      zMax: water.zMax + 2.2,
    },
    pierFoot: { x: water.xMax + 0.35, z },
    pierEnd: { x: water.xMax - 1.2, z },
    exit: { x: visible.xMin - 1.2, z: z - 0.4 },
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

function within(r: Region, p: Vec2) {
  return p.x >= r.xMin && p.x <= r.xMax && p.z >= r.zMin && p.z <= r.zMax;
}

/** Lounger rows, far enough apart for a compact name tag over each. */
const LOUNGER_ROW = 1.8;
/** How far the lounger's root point sits from the pool's edge. */
const LOUNGER_OFFSET = 1;
/** Below this the lounge has room for loungers down one side of the pool only. */
const TWO_SIDED_WIDTH = 4.2;
const FLOATS = 3;

/**
 * A pool down the middle of a tiled deck, loungers facing the viewer down
 * either side of it, each with a drink on a table beside it, and rings on the
 * water for sleepers to drift on.
 */
function lounge(area: Region): Lounge {
  const width = area.xMax - area.xMin;
  const twoSided = width >= TWO_SIDED_WIDTH;
  const half = Math.min(1.1, Math.max(0.45, width * 0.17));
  const cx = twoSided ? (area.xMin + area.xMax) / 2 : area.xMin + half + 0.2;
  const pool = {
    xMin: cx - half,
    xMax: cx + half,
    zMin: area.zMin + 1,
    zMax: area.zMax - 0.8,
  };
  const columns = (
    twoSided
      ? [pool.xMin - LOUNGER_OFFSET, pool.xMax + LOUNGER_OFFSET]
      : [pool.xMax + LOUNGER_OFFSET]
  ).map((x) => Math.min(area.xMax - 0.3, Math.max(area.xMin + 0.3, x)));
  const rows: number[] = [];
  for (let z = pool.zMin + 0.7; z <= pool.zMax; z += LOUNGER_ROW) rows.push(z);
  const middle = (pool.zMin + pool.zMax) / 2;
  // Middle rows first, so a few loungers in use sit together. Each is turned
  // most of the way to the pool, and no two quite alike.
  const seats: Seat[] = rows
    .sort((a, b) => Math.abs(a - middle) - Math.abs(b - middle))
    .flatMap((z) =>
      columns.map((x) => {
        const j = Math.sin(x * 12.9898 + z * 78.233) * 43758.5453;
        const jitter = (j - Math.floor(j) - 0.5) * 0.4;
        return { x, z, facing: (x < cx ? 0.75 : -0.75) + jitter };
      }),
    );
  // On the side away from the pool, by the lounger's head.
  const tables = seats.map((p) => onSeat(p, p.x < cx ? -0.42 : 0.42, -0.35));
  const length = pool.zMax - pool.zMin;
  const naps = Array.from({ length: FLOATS }, (_, i) => ({
    x: cx + (i % 2 ? 0.25 : -0.25) * half,
    z: pool.zMin + ((i + 0.5) * length) / FLOATS,
  }));
  // In the back corner away from the portal.
  const parasols = [{ x: area.xMax - 0.35, z: area.zMin + 0.3 }];
  return { area, pool, seats, tables, naps, parasols };
}

/** A point beside a lounger, given across and along it from its sitter. */
export function onSeat(seat: Seat, across: number, along: number): Vec2 {
  const cos = Math.cos(seat.facing);
  const sin = Math.sin(seat.facing);
  return {
    x: seat.x + across * cos + along * sin,
    z: seat.z - across * sin + along * cos,
  };
}

/** Two whiteboards along the back and two crate piles at the sides. */
function workshop(area: Region): Workshop {
  const cx = (area.xMin + area.xMax) / 2;
  const quarter = (area.xMax - area.xMin) / 4;
  const pileZ = (area.zMin + area.zMax) / 2 + 0.6;
  return {
    area,
    boards: [
      { x: cx - quarter, z: area.zMin + 1.2 },
      { x: cx + quarter, z: area.zMin + 1.2 },
    ],
    piles: [
      { x: area.xMin + 0.4, z: pileZ },
      { x: area.xMax - 0.4, z: pileZ },
    ],
  };
}
