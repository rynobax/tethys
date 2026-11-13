/**
 * The camera is fixed and orthographic, looking straight ahead and down, so
 * screen x maps linearly to world x and screen y to world z. That's what lets
 * the meadows be measured off the terminal panel's own rectangle.
 */

import type { Layout, Pitch, Region, Vec2 } from "./sim";

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
/** Pitchers face the viewer, so a row needs only a name tag's height. */
const STAGE_ROW_PX = 104;
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
      pitch: right ? pitch(right) : null,
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
 * The bottom of the right meadow: a stage at the back where Shipping residents
 * pitch, and a crowd in front of it with its back to the viewer. Front row
 * first, middle out.
 */
function pitch(region: Region): Pitch | null {
  if (region.zMax - region.zMin < MIN_PITCH_DEPTH) return null;
  const colGap = SLOT_WIDTH_PX / PIXELS_PER_UNIT;
  const rowGap = STAGE_ROW_PX / (PIXELS_PER_UNIT * Math.sin(PITCH));
  const front = region.zMax - 2.6;
  const back = front - rowGap;
  const width = region.xMax - region.xMin;
  const centre = (region.xMin + region.xMax) / 2;

  // As many tag-widths as fit across, centred; the back row sits between them.
  const n = Math.max(1, Math.floor(width / colGap) + 1);
  const xs = Array.from(
    { length: n },
    (_, i) => centre + (i - (n - 1) / 2) * colGap,
  );
  const between = n > 1 ? xs.slice(1).map((x, i) => (x + xs[i]) / 2) : xs;
  const middleOut = (a: Vec2, b: Vec2) =>
    Math.abs(a.x - centre) - Math.abs(b.x - centre);
  const slots = [
    ...xs.map((x) => ({ x, z: front })).sort(middleOut),
    ...between.map((x) => ({ x, z: back })).sort(middleOut),
  ];

  const stage = {
    xMin: region.xMin - 0.6,
    xMax: region.xMax + 0.6,
    zMin: back - 0.9,
    zMax: front + 0.7,
  };
  const crowd: Vec2[] = [];
  for (const [z, shift] of [
    [region.zMax + 0.2, 0],
    [region.zMax - 0.55, 0.4],
  ] as const) {
    for (let x = stage.xMin + 0.4 + shift; x < stage.xMax - 0.2; x += 0.8) {
      // Deterministic jitter, so a resize doesn't reshuffle the crowd much.
      const j = Math.sin(x * 12.9898 + z * 78.233) * 43758.5453;
      crowd.push({ x: x + (j - Math.floor(j) - 0.5) * 0.25, z });
    }
  }
  return { slots, crowd, stage, top: stage.zMin - 0.4 };
}
