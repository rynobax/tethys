import type { Environment } from "./environment";

/**
 * The colours and light of everything that isn't a resident. Each
 * environment's props read what they need of it: `foliage` is whatever lines
 * the back, `flowers` whatever's dotted about.
 */
export interface Palette {
  ground: string;
  patch: string;
  sky: string;
  hemiGround: string;
  hemiIntensity: number;
  sun: string;
  sunIntensity: number;
  foliage: string[];
  trunk: string;
  rock: string;
  flowers: string[];
  portal: string;
  /** The water in the lounge's pool and the lake. */
  water: string;
  /** How much the water lights itself, for an evening swim. */
  waterGlow: number;
  fireflies: boolean;
}

export const MEADOW: Palette = {
  ground: "#a9d673",
  patch: "#9ccb64",
  sky: "#eef8ff",
  hemiGround: "#8fbf5a",
  hemiIntensity: 1.6,
  sun: "#fff3dc",
  sunIntensity: 1.9,
  foliage: ["#5fb35a", "#4fa152", "#77c26a"],
  trunk: "#9a6b47",
  rock: "#bdb8b0",
  flowers: ["#ff8fab", "#ffd166", "#ffffff", "#b39ddb"],
  portal: "#9b6bff",
  water: "#4fc3e8",
  waterGlow: 0.05,
  fireflies: false,
};

export const DUSK: Palette = {
  ground: "#3e6a4e",
  patch: "#376146",
  sky: "#9fb2e0",
  hemiGround: "#2c4a3a",
  hemiIntensity: 1.6,
  sun: "#ffc98f",
  sunIntensity: 1.6,
  foliage: ["#2f6b4a", "#285e41", "#3a7a55"],
  trunk: "#5d4636",
  rock: "#7c7f86",
  flowers: ["#f48fb1", "#ffe082", "#b39ddb"],
  portal: "#b388ff",
  water: "#2a9fd0",
  waterGlow: 0.45,
  fireflies: true,
};

/** Regolith under a hard sun, and by earthlight. */
const MOON_DAY: Palette = {
  ground: "#c4c3c8",
  patch: "#b3b2b9",
  sky: "#f2f4ff",
  hemiGround: "#8e8d96",
  hemiIntensity: 1.3,
  sun: "#ffffff",
  sunIntensity: 2.3,
  foliage: ["#eceef3", "#d9dde5", "#f5f5f8"],
  trunk: "#7d838f",
  rock: "#9d9ca4",
  flowers: ["#6fe3ff", "#b48cff", "#7dffc4", "#ffffff"],
  portal: "#3fd6ff",
  water: "#5f86d8",
  waterGlow: 0.15,
  fireflies: false,
};

const MOON_NIGHT: Palette = {
  ground: "#4d4f5c",
  patch: "#43454f",
  sky: "#5d6aa8",
  hemiGround: "#2c2e38",
  hemiIntensity: 1.5,
  sun: "#bcd0ff",
  sunIntensity: 1.5,
  foliage: ["#b9bfcc", "#a3aaba", "#c8cdd8"],
  trunk: "#5d6370",
  rock: "#6a6b76",
  flowers: ["#6fe3ff", "#b48cff", "#7dffc4", "#ffffff"],
  portal: "#3fd6ff",
  water: "#4a62c8",
  waterGlow: 0.4,
  fireflies: false,
};

/** Dry summer grass, olives and cypresses, marble. */
const ATHENS_DAY: Palette = {
  ground: "#d8cd8c",
  patch: "#cdbf78",
  sky: "#eaf5ff",
  hemiGround: "#b5a764",
  hemiIntensity: 1.6,
  sun: "#fff0d2",
  sunIntensity: 2.0,
  foliage: ["#3d6a3c", "#8a9f73", "#4f7b47"],
  trunk: "#8a6a4a",
  rock: "#f1ece0",
  flowers: ["#e5412f", "#f6f2e6", "#8a5cc2", "#e8b23a"],
  portal: "#3a8fe8",
  water: "#2f86d6",
  waterGlow: 0.05,
  fireflies: false,
};

const ATHENS_NIGHT: Palette = {
  ground: "#6c6a4a",
  patch: "#625f42",
  sky: "#a4b0de",
  hemiGround: "#45432f",
  hemiIntensity: 1.6,
  sun: "#ffc890",
  sunIntensity: 1.6,
  foliage: ["#284a2e", "#5d6e52", "#355a37"],
  trunk: "#5d4636",
  rock: "#c9c4b8",
  flowers: ["#d8503f", "#e8e2d0", "#9a74cf", "#e0b04a"],
  portal: "#5aa6ff",
  water: "#2272c0",
  waterGlow: 0.4,
  fireflies: true,
};

/** Carpet tiles under strip lights, and after hours. */
const OFFICE_DAY: Palette = {
  ground: "#a3adbb",
  patch: "#97a2b1",
  sky: "#f6f8fc",
  hemiGround: "#9199a6",
  hemiIntensity: 1.8,
  sun: "#ffffff",
  sunIntensity: 1.5,
  foliage: ["#3f8f4a", "#57a85a", "#2f7a3f"],
  trunk: "#ece9e2",
  rock: "#c99d66",
  flowers: ["#ffe066", "#ff9fb2", "#9fe0ff", "#b8f27a"],
  portal: "#7fd6ff",
  water: "#3d9ec4",
  waterGlow: 0.05,
  fireflies: false,
};

const OFFICE_NIGHT: Palette = {
  ground: "#3f4656",
  patch: "#394050",
  sky: "#8c9ac4",
  hemiGround: "#2a2f3a",
  hemiIntensity: 1.6,
  sun: "#d8e2ff",
  sunIntensity: 1.3,
  foliage: ["#2a6638", "#3a7a45", "#215a30"],
  trunk: "#b9b6ae",
  rock: "#8f7150",
  flowers: ["#e8cc5a", "#e08c9c", "#8cc8e6", "#a2d86c"],
  portal: "#7fd6ff",
  water: "#2a7ea6",
  waterGlow: 0.35,
  fireflies: false,
};

export const PALETTES: Record<Environment, { light: Palette; dark: Palette }> =
  {
    meadow: { light: MEADOW, dark: DUSK },
    moon: { light: MOON_DAY, dark: MOON_NIGHT },
    athens: { light: ATHENS_DAY, dark: ATHENS_NIGHT },
    office: { light: OFFICE_DAY, dark: OFFICE_NIGHT },
  };
