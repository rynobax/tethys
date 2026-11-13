#!/usr/bin/env node
// Screenshots of fun mode's preview at exact moments, for iterating on the
// look. Drives headless Chrome over the DevTools protocol with no
// dependencies, and the world through the preview's manual clock
// (`fun-preview.html?shot`), so a shot is of a chosen sim time rather than of
// however many frames a slow software renderer happened to draw. Random
// numbers are seeded, so the same commands give the same picture.
//
//   node scripts/fun-shot.mjs [options] <command>...
//
// Commands, run in order:
//   wait:SECONDS            run the world on that long
//   mood:ID=MOOD            change a sample's mood (needs_you, working, idle,
//                           asleep, shipping, shipped, queued, creating, failed)
//   remove:ID               delete a sample
//   progress:ID=DONE/TOTAL  set a sample's task count
//   fold:FOLDER=1|0         shut (1) or open (0) a folder: lint, spikes, meta
//   select:ID               select a sample (`select:` for none)
//   shot:FILE[@X,Y,W,H][xSCALE]
//                           save a PNG, of a clip of the window in CSS pixels
//                           if given, at SCALE times (default 1)
//   ids                     print the samples
//   eval:EXPR               print a page expression, e.g. about
//                           `window.__fun.sim.agents`
//   pose:ACTIVITY[,FACING]  with --gallery: every animal in that pose (walk,
//                           work, sleep, seated, lounge, garden, attention, ...)
//
// Options: --width 1728 --height 1000 --dark --seed N --port 5199
// --gallery shows every animal, grown up and as a child, instead of the world.
// --query "zoom=200&only=fox" adds to the page's query: the gallery takes
// zoom, only (animals, comma separated), pose and facing.
// The preview's dev server is started on --port if nothing answers there.

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

const args = process.argv.slice(2);
const opts = { width: 1728, height: 1000, dark: false, seed: 1, port: 5199 };
const commands = [];
for (let i = 0; i < args.length; i++) {
  const a = args[i];
  if (a === "--dark") opts.dark = true;
  else if (a === "--gallery") opts.gallery = true;
  else if (a === "--query") opts.query = args[++i];
  else if (a.startsWith("--")) opts[a.slice(2)] = Number(args[++i]);
  else commands.push(a);
}
if (commands.length === 0) {
  console.error(
    readFileSync(new URL(import.meta.url), "utf8")
      .split("\n")
      .slice(1, 30)
      .join("\n"),
  );
  process.exit(1);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const cleanups = [];
const cleanup = () => {
  for (const fn of cleanups.reverse()) {
    try {
      fn();
    } catch {}
  }
};
process.on("exit", cleanup);
process.on("SIGINT", () => process.exit(130));

async function answers(url) {
  try {
    return (await fetch(url)).ok;
  } catch {
    return false;
  }
}

async function ensureServer() {
  const url = `http://localhost:${opts.port}/fun-preview.html`;
  if (await answers(url)) return;
  const vite = spawn(
    "pnpm",
    ["vite", "--port", String(opts.port), "--strictPort"],
    {
      stdio: "ignore",
      detached: true,
    },
  );
  cleanups.push(() => process.kill(-vite.pid));
  for (let i = 0; i < 100; i++) {
    if (await answers(url)) return;
    await sleep(100);
  }
  throw new Error(`no dev server on ${opts.port}`);
}

async function launchChrome() {
  const profile = mkdtempSync(join(tmpdir(), "fun-shot-"));
  cleanups.push(() => rmSync(profile, { recursive: true, force: true }));
  const chrome = spawn(
    CHROME,
    [
      "--headless=new",
      "--remote-debugging-port=0",
      `--user-data-dir=${profile}`,
      "--use-angle=swiftshader",
      "--enable-unsafe-swiftshader",
      "--hide-scrollbars",
      "--mute-audio",
      "--no-first-run",
      "about:blank",
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  cleanups.push(() => chrome.kill("SIGKILL"));
  const port = await new Promise((resolve, reject) => {
    let err = "";
    chrome.stderr.on("data", (d) => {
      err += d;
      const m = err.match(/DevTools listening on ws:\/\/[^:]+:(\d+)\//);
      if (m) resolve(Number(m[1]));
    });
    chrome.on("exit", () => reject(new Error(`chrome exited:\n${err}`)));
  });
  const pages = await (
    await fetch(`http://127.0.0.1:${port}/json/list`)
  ).json();
  const page = pages.find((p) => p.type === "page");
  return connect(page.webSocketDebuggerUrl);
}

function connect(url) {
  const ws = new WebSocket(url);
  let id = 0;
  const pending = new Map();
  const listeners = new Map();
  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(msg.error.message));
      else resolve(msg.result);
    } else if (msg.method) {
      for (const fn of listeners.get(msg.method) ?? []) fn(msg.params);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      pending.set(++id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  const on = (method, fn) => {
    if (!listeners.has(method)) listeners.set(method, []);
    listeners.get(method).push(fn);
  };
  return new Promise((resolve) => {
    ws.onopen = () => resolve({ send, on });
  });
}

async function evaluate(cdp, expression) {
  const { result, exceptionDetails } = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (exceptionDetails) {
    throw new Error(
      exceptionDetails.exception?.description ?? exceptionDetails.text,
    );
  }
  return result.value;
}

// Two frames, so what was drawn has reached the screen before it's captured.
const PRESENTED =
  "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))";

async function main() {
  const started = Date.now();
  await ensureServer();
  const cdp = await launchChrome();
  const errors = [];
  cdp.on("Runtime.exceptionThrown", (p) =>
    errors.push(
      p.exceptionDetails.exception?.description ?? p.exceptionDetails.text,
    ),
  );
  cdp.on("Runtime.consoleAPICalled", (p) => {
    if (p.type === "error")
      errors.push(p.args.map((a) => a.value ?? a.description).join(" "));
  });
  await cdp.send("Runtime.enable");
  await cdp.send("Page.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: opts.width,
    height: opts.height,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await cdp.send("Emulation.setEmulatedMedia", {
    features: [
      { name: "prefers-color-scheme", value: opts.dark ? "dark" : "light" },
    ],
  });
  // Seeded before anything on the page runs.
  await cdp.send("Page.addScriptToEvaluateOnNewDocument", {
    source: `{ let s = ${opts.seed >>> 0} || 1; Math.random = () => {
      s = (s + 0x6d2b79f5) >>> 0; let t = s;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296; }; }`,
  });
  await cdp.send("Page.navigate", {
    url: `http://localhost:${opts.port}/fun-preview.html?shot${opts.gallery ? "&gallery" : ""}${opts.query ? `&${opts.query}` : ""}`,
  });
  for (let i = 0; ; i++) {
    if (await evaluate(cdp, "!!window.__fun").catch(() => false)) break;
    if (i > 300)
      throw new Error(`the preview never came up\n${errors.join("\n")}`);
    await sleep(50);
  }
  // The first frame, so avatars loaded since startup are drawn.
  await evaluate(cdp, "window.__fun.step(0.05)");

  for (const command of commands) {
    const [verb, rest = ""] = command.split(/:(.*)/s);
    const t0 = Date.now();
    if (verb === "wait") {
      await evaluate(cdp, `window.__fun.step(${Number(rest)})`);
    } else if (verb === "mood") {
      const [id, mood] = rest.split("=");
      await evaluate(
        cdp,
        `window.__fun.mood(${JSON.stringify(id)}, ${JSON.stringify(mood)})`,
      );
      await evaluate(cdp, "window.__fun.step(0.05)");
    } else if (verb === "fold") {
      const [id, shut] = rest.split("=");
      await evaluate(
        cdp,
        `window.__fun.fold(${JSON.stringify(id)}, ${shut === "1"})`,
      );
      await evaluate(cdp, "window.__fun.step(0.05)");
    } else if (verb === "progress") {
      const [id, count] = rest.split("=");
      const [done, total] = count.split("/").map(Number);
      await evaluate(
        cdp,
        `window.__fun.progress(${JSON.stringify(id)}, ${done}, ${total})`,
      );
      await evaluate(cdp, "window.__fun.step(0.05)");
    } else if (verb === "remove") {
      await evaluate(cdp, `window.__fun.remove(${JSON.stringify(rest)})`);
      await evaluate(cdp, "window.__fun.step(0.05)");
    } else if (verb === "select") {
      await evaluate(
        cdp,
        `window.__fun.select(${rest ? JSON.stringify(rest) : "null"})`,
      );
      await evaluate(cdp, "window.__fun.step(0.05)");
    } else if (verb === "pose") {
      const [activity, facing = "0.35"] = rest.split(",");
      await evaluate(
        cdp,
        `window.__fun.pose(${JSON.stringify(activity)}, ${Number(facing)})`,
      );
      await evaluate(cdp, "window.__fun.step(0.05)");
    } else if (verb === "eval") {
      console.log(JSON.stringify(await evaluate(cdp, rest)));
      continue;
    } else if (verb === "ids") {
      console.log((await evaluate(cdp, "window.__fun.ids()")).join("\n"));
    } else if (verb === "shot") {
      const m = rest.match(
        /^(.+?\.png)(?:@(\d+),(\d+),(\d+),(\d+))?(?:x([\d.]+))?$/,
      );
      if (!m) throw new Error(`bad shot: ${rest}`);
      const [, file, x, y, w, h, scale = "1"] = m;
      await evaluate(cdp, PRESENTED);
      const clip = x
        ? { x: +x, y: +y, width: +w, height: +h, scale: +scale }
        : { x: 0, y: 0, width: opts.width, height: opts.height, scale: +scale };
      const { data } = await cdp.send("Page.captureScreenshot", {
        format: "png",
        clip,
        captureBeyondViewport: false,
      });
      writeFileSync(file, Buffer.from(data, "base64"));
      console.log(`${file} (${Date.now() - t0}ms)`);
      continue;
    } else {
      throw new Error(`unknown command: ${command}`);
    }
    console.log(`${command} (${Date.now() - t0}ms)`);
  }
  if (errors.length) console.error(`page errors:\n${errors.join("\n")}`);
  console.log(`done in ${Date.now() - started}ms`);
}

main()
  .then(() => process.exit(0))
  .catch((e) => {
    console.error(e.message);
    process.exit(1);
  });
