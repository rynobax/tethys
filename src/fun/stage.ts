/**
 * The camera is fixed and orthographic, looking straight ahead and down, so
 * screen x maps linearly to world x and screen y to world z. That's what lets
 * the meadows be measured off the terminal panel's own rectangle.
 */

import type { Layout, Lounge, Pitch, Region, Vec2, Workshop } from "./sim";

/** Pixels per world unit; a top-level resident is about one unit tall. */
export const PIXELS_PER_UNIT = 54;
export const PITCH = (40 * Math.PI) / 180;
export const CAMERA_DISTANCE = 40;

/** Room for half a name tag, so none runs off the window. */
const EDGE_MARGIN = 1.6;
/** Room for half a name tag, so none slides under the panel. */
const PANEL_MARGIN = 1.4;
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
const CHAIR_ROWS = 2;
/** Below this the right meadow is too short to give any of it to a stage. */
const MIN_PITCH_DEPTH = 12;

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
  const portal = (r: Region | null) =>
    r ? { x: (r.xMin + r.xMax) / 2, z: r.zMin - 0.2 } : null;

  return {
    layout: {
      regions: { left, right },
      portals: { left: portal(left), right: portal(right) },
      attentionSlots: left ? attentionSlots(left) : [],
      ...rightZones(right),
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
 * The bottom of the right meadow: a stage where one Shipping resident pitches
 * at a time from the front, the rest waiting in rows of chairs behind it, and
 * a crowd in front with its back to the viewer.
 */
function pitch(region: Region): Pitch | null {
  if (region.zMax - region.zMin < MIN_PITCH_DEPTH) return null;
  const sinPitch = Math.sin(PITCH);
  const colGap = CHAIR_WIDTH_PX / PIXELS_PER_UNIT;
  const rowGap = CHAIR_ROW_PX / (PIXELS_PER_UNIT * sinPitch);
  const front = region.zMax - 1.9;
  const width = region.xMax - region.xMin;
  const centre = (region.xMin + region.xMax) / 2;
  const middleOut = (a: Vec2, b: Vec2) =>
    Math.abs(a.x - centre) - Math.abs(b.x - centre);

  // As many tag-widths as fit across, centred, each row offset half a chair
  // from the one in front so no tag sits straight above another.
  const n = Math.max(1, Math.floor(width / colGap) + 1);
  const chairs: Vec2[] = [];
  for (let r = 0; r < CHAIR_ROWS; r++) {
    const z = front - (r + 1) * rowGap;
    const count = r % 2 ? Math.max(1, n - 1) : n;
    const row = Array.from({ length: count }, (_, i) => ({
      x: centre + (i - (count - 1) / 2) * colGap,
      z,
    }));
    chairs.push(...row.sort(middleOut));
  }
  const back = front - CHAIR_ROWS * rowGap;

  const stage = {
    xMin: region.xMin - 0.6,
    xMax: region.xMax + 0.6,
    zMin: back - 0.7,
    zMax: front + 0.6,
  };
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
    crowd,
    stage,
    top: stage.zMin - 0.4,
  };
}

/**
 * The right meadow from the back: the lounge, then the workshop down to the
 * front edge, with the stage taking the bottom of the workshop while it's up.
 */
function rightZones(
  region: Region | null,
): Pick<Layout, "lounge" | "workshop" | "pitch"> {
  if (!region || region.zMax - region.zMin < MIN_PITCH_DEPTH) {
    return { lounge: null, workshop: null, pitch: null };
  }
  const stage = pitch(region);
  const bottom = stage?.top ?? region.zMax;
  const loungeDepth = Math.min(7.5, Math.max(5, (bottom - region.zMin) * 0.42));
  const loungeArea = { ...region, zMax: region.zMin + loungeDepth };
  const workshopArea = { ...region, zMin: loungeArea.zMax + 0.6 };
  return {
    lounge: lounge(loungeArea),
    workshop: workshop(workshopArea, bottom),
    pitch: stage,
  };
}

/** A fire in the middle, log seats in a ring round it, bedrolls in the corners. */
function lounge(area: Region): Lounge {
  const fire = {
    x: (area.xMin + area.xMax) / 2,
    z: (area.zMin + area.zMax) / 2 + 0.3,
  };
  const rx = Math.min(2.1, (area.xMax - area.xMin) / 2 - 0.2);
  const rz = Math.min(2.3, (area.zMax - area.zMin) / 2 - 0.4);
  // Starting off the vertical, so no seat is straight behind or before the fire.
  const seats = Array.from({ length: 6 }, (_, i) => {
    const angle = Math.PI / 6 + (i * Math.PI) / 3;
    return {
      x: fire.x + Math.cos(angle) * rx,
      z: fire.z + Math.sin(angle) * rz,
    };
  });
  const naps = [
    { x: area.xMin + 0.3, z: area.zMin + 0.9 },
    { x: area.xMax - 0.3, z: area.zMin + 0.9 },
    { x: area.xMin + 0.3, z: area.zMax - 0.2 },
    { x: area.xMax - 0.3, z: area.zMax - 0.2 },
  ];
  return { area, fire, seats, naps };
}

/**
 * Two whiteboards along the back and two crate piles at the sides, all above
 * where the stage would reach, so they're there whether or not it's up.
 */
function workshop(area: Region, stageTop: number): Workshop {
  const cx = (area.xMin + area.xMax) / 2;
  const quarter = (area.xMax - area.xMin) / 4;
  const pileZ = Math.min((area.zMin + stageTop) / 2, stageTop - 1);
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
