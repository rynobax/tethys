import {
  app,
  type MenuItemConstructorOptions,
  type WebContents,
} from "electron";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

/** Half a level, as the stock zoom roles step. A level is a factor of 1.2. */
const STEP = 0.5;
const MIN = -3;
const MAX = 5;

/** Kept with Electron's profile: it's this machine's screen, not Tethys
 *  state. Chromium keeps no zoom for a `file://` page. */
const file = () => path.join(app.getPath("userData"), "zoom.json");

/** The zoom level saved last, for the window's first paint. */
export function savedZoomFactor(): number {
  try {
    const { level } = JSON.parse(readFileSync(file(), "utf8"));
    if (typeof level === "number") return Math.pow(1.2, clamp(level));
  } catch {
    // None saved yet.
  }
  return 1;
}

function clamp(level: number) {
  return Math.min(MAX, Math.max(MIN, level));
}

/** The View menu's zoom items, in place of the stock roles so a change is
 *  saved. */
export function zoomMenuItems(
  contents: WebContents,
): MenuItemConstructorOptions[] {
  const set = (level: number) => {
    contents.setZoomLevel(clamp(level));
    writeFileSync(
      file(),
      JSON.stringify({ level: contents.getZoomLevel() }) + "\n",
    );
  };
  const by = (step: number) => () => set(contents.getZoomLevel() + step);
  return [
    {
      label: "Actual Size",
      accelerator: "CommandOrControl+0",
      click: () => set(0),
    },
    {
      label: "Zoom In",
      accelerator: "CommandOrControl+=",
      click: by(STEP),
    },
    // Cmd+Shift+= too, as the stock role takes.
    {
      label: "Zoom In",
      accelerator: "CommandOrControl+Plus",
      visible: false,
      acceleratorWorksWhenHidden: true,
      click: by(STEP),
    },
    {
      label: "Zoom Out",
      accelerator: "CommandOrControl+-",
      click: by(-STEP),
    },
  ];
}
