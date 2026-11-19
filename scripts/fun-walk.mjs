#!/usr/bin/env node
// Whether fun mode's residents get where they're going. Runs the real sim
// headless (bundled with esbuild, no browser) and exits non-zero on any
// failure, so a change to the layout or to how residents move can be checked
// in seconds.
//
//   node scripts/fun-walk.mjs [--quick] [--seeds 1,2,3] [--minutes 20] [-v]
//
// Two runs:
//   reach  One resident alone on the meadow, walked from every named spot
//          (attention slots, lines, doorsteps, chairs, loungers, plots, ...)
//          to every other, at every size and at a few window sizes. Fails
//          when it never arrives.
//   churn  24 residents whose moods, folders and selection change every few
//          seconds, far harsher than real use. Fails when anyone settles
//          short of its spot, walks over 20 s at one target, jumps across
//          the ground other than by door or portal, or the sim warns.
//
// --quick samples a tenth of the reach pairs and one seed of churn.

import { build } from "esbuild";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const quick = flag("--quick");
const verbose = flag("-v");
const seeds = opt("--seeds", quick ? "1" : "1,2,3")
  .split(",")
  .map(Number);
const minutes = Number(opt("--minutes", quick ? 10 : 20));

const root = fileURLToPath(new URL("..", import.meta.url));
const bundled = await build({
  stdin: {
    contents:
      'export { Sim } from "./src/fun/sim.ts";\n' +
      'export { computeStage } from "./src/fun/stage.ts";\n',
    resolveDir: root,
    loader: "ts",
  },
  bundle: true,
  format: "esm",
  platform: "node",
  write: false,
  logLevel: "error",
});
const dir = mkdtempSync(join(tmpdir(), "fun-walk-"));
const file = join(dir, "sim.mjs");
writeFileSync(file, bundled.outputFiles[0].text);
const { Sim, computeStage } = await import(pathToFileURL(file).href);
rmSync(dir, { recursive: true });

function seed(n) {
  let s = n >>> 0 || 1;
  Math.random = () => {
    s ^= s << 13;
    s >>>= 0;
    s ^= s >>> 17;
    s ^= s << 5;
    s >>>= 0;
    return s / 4294967296;
  };
}
const dist = (a, b) => Math.hypot(a.x - b.x, a.z - b.z);
const round = (n) => Math.round(n * 100) / 100;
const at = (p) => `(${round(p.x)}, ${round(p.z)})`;
const DT = 1 / 60;

// The sim warns when it has to rescue someone; each is a failure here.
const warnings = [];
console.warn = (...xs) => warnings.push(xs.join(" "));

function stage(w, h, folders = 3) {
  // The terminal's panel in the middle, as in the preview.
  const left = Math.round(w * 0.26);
  return computeStage(w, h, { left, right: w - left }, folders).layout;
}

function resident(id, mood, extra = {}) {
  return {
    id,
    branch: id,
    mood,
    parentId: null,
    depth: 0,
    selected: false,
    animal: "fox",
    work: "x",
    progress: null,
    note: null,
    readyToMerge: mood === "ready",
    quilts: false,
    home: null,
    ...extra,
  };
}

/** Every place someone is sent to stand, by side. */
function namedSpots(L) {
  const out = { left: [], right: [] };
  const add = (side, name, p) => p && out[side].push({ name, p });
  // A spread of the waiting ground, which is too fine to walk all of.
  const ground = L.waiting?.spots ?? [];
  const every = Math.max(1, Math.ceil(ground.length / 16));
  ground.forEach((p, i) => i % every || add("left", `wait${i}`, p));
  L.lake?.tickets.forEach((p, i) => add("left", `ticket${i}`, p));
  L.lake?.dock.forEach((p, i) => add("left", `dock${i}`, p));
  add("left", "pierFoot", L.lake?.pierFoot);
  add("left", "pierEnd", L.lake?.pierEnd);
  L.houses.forEach((h, i) => {
    add("left", `door${i}`, h.door);
    h.doorstep.forEach((p, j) => add("left", `step${i}.${j}`, p));
  });
  add("left", "portal", L.portals.left);
  L.pitch?.chairs.forEach((p, i) => add("left", `chair${i}`, p));
  add("left", "podium", L.pitch?.podium);
  add("right", "portal", L.portals.right);
  L.lounge?.seats.forEach((p, i) => add("right", `seat${i}`, p));
  L.lounge?.naps.forEach((p, i) => add("right", `ring${i}`, p));
  L.workshop?.boards.forEach((p, i) => add("right", `board${i}`, p));
  L.workshop?.garden.plots.forEach((p, i) =>
    add("right", `plot${i}`, { x: p.x, z: p.z - 0.42 }),
  );
  const store = L.workshop?.garden.store;
  if (store) {
    for (const ph of [0, 1.57, 3.14]) {
      add("right", `store${ph}`, {
        x: store.x + Math.cos(ph) * 0.35,
        z: store.z + 0.45 + Math.abs(Math.sin(ph)) * 0.2,
      });
    }
  }
  L.porch?.places.forEach((p, i) => add("right", `quilt${i}`, p.seat));
  return out;
}

/** Whether `p` is on water, a house or a pool, or in the furniture, other
 *  than what the walk starts or ends in. */
function trespass(L, p, ends) {
  const into = (r, q) =>
    q.x > r.xMin && q.x < r.xMax && q.z > r.zMin && q.z < r.zMax;
  const pier = L.lake && {
    xMin: L.lake.pierEnd.x - 0.15,
    xMax: L.lake.pierFoot.x + 0.6,
    zMin: L.lake.pierEnd.z - 0.22,
    zMax: L.lake.pierEnd.z + 0.22,
  };
  if (pier && into(pier, p)) return false;
  for (const b of L.blocks) {
    if (into(b, p) && !ends.some((e) => into(b, e))) return true;
  }
  for (const o of L.obstacles) {
    const r = o.r * 0.8;
    if (dist(o, p) < r && !ends.some((e) => dist(o, e) < o.r + 0.6)) {
      return true;
    }
  }
  return false;
}

/** One walker, nobody else, from every named spot to every other. */
function reach(w, h) {
  seed(7);
  const L = stage(w, h);
  const spots = namedSpots(L);
  const fails = [];
  const crossed = [];
  let pairs = 0;
  let k = 0;
  for (const size of [0.7, 0.455, 0.315]) {
    for (const side of ["left", "right"]) {
      for (const A of spots[side]) {
        for (const B of spots[side]) {
          if (A === B || dist(A.p, B.p) < 0.3) continue;
          if (quick && k++ % 10 !== 0) continue;
          pairs++;
          const sim = new Sim();
          sim.setLayout(L);
          sim.sync([resident("w", "idle")]);
          const a = sim.agents.get("w");
          a.side = side;
          a.size = size;
          a.pos = { ...A.p };
          a.activity = "pause";
          sim.walkTo(a, { ...B.p }, "wander");
          const limit = (dist(A.p, B.p) * 4) / 1.5 + 6;
          let ok = false;
          let across = null;
          for (let t = 0; t < limit; t += DT) {
            sim.now += DT;
            sim.dt = DT;
            sim.move(a, DT, 1.5);
            if (!across && trespass(L, a.pos, [A.p, B.p])) {
              across = { ...a.pos };
            }
            if (dist(a.pos, B.p) < 0.2) {
              ok = true;
              break;
            }
          }
          if (!ok) fails.push({ size, side, A, B, end: { ...a.pos } });
          if (across) crossed.push({ size, side, A, B, end: across });
        }
      }
    }
  }
  return { pairs, fails, crossed };
}

const MOODS = [
  "needs_you",
  "working",
  "idle",
  "shipping",
  "ready",
  "shipped",
  "working",
  "idle",
  "needs_you",
];
const SHIPS = new Set(["shipping", "ready", "shipped"]);
// Where a resident stops and stays. Not seats, rings and the quilting chairs,
// which it's put on when it gets there: a big step onto one is a jump.
const POSTS = new Set([
  "attention",
  "work",
  "pitch",
  "board",
  "garden",
  "queue",
]);
const DOORS = new Set([
  "portal_in",
  "portal_out",
  "home",
  "home_in",
  "home_out",
  "sail",
]);

/** A crowd whose moods, folders and selection keep changing. */
function churn(s) {
  seed(s);
  const L = stage(1728, 1000);
  const sim = new Sim();
  sim.setLayout(L);
  const rs = [];
  for (let i = 0; i < 24; i++) {
    const parent = i % 6 === 1 ? rs[i - 1] : null;
    rs.push(
      resident(`r${i}`, MOODS[i % MOODS.length], {
        parentId: parent?.id ?? null,
        depth: parent ? 1 : 0,
        quilts: i % 6 === 0,
      }),
    );
  }
  sim.sync(rs);
  // Every fixed place to stop at: a line moving up has one stop at the place
  // ahead, nearer than where it was making for.
  const places = [...namedSpots(L).left, ...namedSpots(L).right].map(
    (s) => s.p,
  );
  const onPlace = (p) =>
    [...places, ...sim.waitSpots.values()].some((q) => dist(p, q) <= 0.25);
  const found = { short: [], forever: [], jump: [] };
  const seen = new Map();
  const shut = [false, false, false];
  let next = 2;
  const steps = Math.round((minutes * 60) / DT);
  for (let k = 0; k < steps; k++) {
    if (sim.time() >= next) {
      next = sim.time() + 4 * (0.5 + Math.random());
      const i = Math.floor(Math.random() * rs.length);
      const roll = Math.random();
      if (roll < 0.75) {
        const mood = MOODS[Math.floor(Math.random() * MOODS.length)];
        rs[i] = { ...rs[i], mood, readyToMerge: mood === "ready" };
      } else if (roll < 0.9) {
        shut[Math.floor(Math.random() * 3)] = Math.random() < 0.5;
      } else {
        for (let j = 0; j < rs.length; j++) {
          rs[j] = { ...rs[j], selected: j === i ? !rs[j].selected : false };
        }
      }
      // As residents.ts has it: home while its folder's shut, unless
      // selected or shipping or shipped.
      for (let j = 0; j < rs.length; j++) {
        const out = rs[j].selected || SHIPS.has(rs[j].mood);
        rs[j] = { ...rs[j], home: shut[j % 3] && !out ? j % 3 : null };
      }
      sim.sync(rs);
    }
    const before = new Map();
    for (const a of sim.agents.values()) {
      before.set(a.id, {
        pos: { ...a.pos },
        activity: a.activity,
        target: a.target && { ...a.target },
      });
    }
    sim.step(DT);
    const now = sim.time();
    for (const a of sim.agents.values()) {
      const b = before.get(a.id);
      if (!b) continue;
      let st = seen.get(a.id);
      if (!st) seen.set(a.id, (st = { since: 0, target: null, told: false }));
      const where = { t: round(now), id: a.id, at: at(a.pos) };
      if (
        b.activity === "walk" &&
        POSTS.has(a.activity) &&
        b.target &&
        dist(a.pos, b.target) > 0.25 &&
        !onPlace(a.pos)
      ) {
        found.short.push({
          ...where,
          post: a.activity,
          d: round(dist(a.pos, b.target)),
        });
      }
      if (a.activity === "walk" && a.target) {
        if (!st.target || dist(st.target, a.target) > 0.3) {
          st.since = now;
          st.target = { ...a.target };
          st.told = false;
        }
        if (now - st.since > 20 && !st.told) {
          st.told = true;
          found.forever.push({ ...where, to: at(a.target), intent: a.intent });
        }
      } else {
        st.target = null;
      }
      const jump = dist(a.pos, b.pos);
      if (jump > 0.35 && !DOORS.has(a.activity) && !DOORS.has(b.activity)) {
        found.jump.push({
          ...where,
          jump: round(jump),
          from: b.activity,
          to: a.activity,
        });
      }
    }
  }
  return found;
}

const t0 = performance.now();
let failed = false;
const report = (label, items, show) => {
  const bad = items.length > 0;
  failed ||= bad;
  console.log(`${bad ? "FAIL" : "ok  "} ${label}: ${items.length}`);
  for (const x of items.slice(0, verbose ? 40 : 5))
    console.log("       " + show(x));
};

for (const [w, h] of [
  [1728, 1000],
  [1440, 900],
  [2560, 1400],
  [1280, 760],
]) {
  const before = warnings.length;
  const { pairs, fails, crossed } = reach(w, h);
  const tag = `reach ${w}x${h}, ${pairs} walks`;
  report(
    `${tag}, never arrived`,
    fails,
    (f) =>
      `${f.side} size ${f.size}: ${f.A.name} -> ${f.B.name}, stopped at ${at(f.end)}, ${round(dist(f.end, f.B.p))} short`,
  );
  report(
    `${tag}, crossed water, a house or furniture`,
    crossed,
    (f) =>
      `${f.side} size ${f.size}: ${f.A.name} -> ${f.B.name}, at ${at(f.end)}`,
  );
  report(`${tag}, sim warnings`, warnings.slice(before), (x) => x);
}
for (const s of seeds) {
  const before = warnings.length;
  const found = churn(s);
  const tag = `churn seed ${s}, ${minutes} min`;
  report(`${tag}, settled short`, found.short, (x) => JSON.stringify(x));
  report(`${tag}, walked >20 s at one target`, found.forever, (x) =>
    JSON.stringify(x),
  );
  report(`${tag}, jumped`, found.jump, (x) => JSON.stringify(x));
  report(`${tag}, sim warnings`, warnings.slice(before), (x) => x);
}
console.log(`${((performance.now() - t0) / 1000).toFixed(1)} s`);
process.exit(failed ? 1 : 0);
