#!/usr/bin/env node
// `pnpm dev`: builds the three Rust binaries, serves the UI on 1430 and runs
// Electron against it, which spawns `target/debug/tethysd`. A renderer reload
// never touches the daemon. Unless frozen (`TETHYS_NO_HMR`), a change under
// `src-tauri/`, `crates/` or `electron/` rebuilds and restarts the app, the
// daemon with it; sessions live on in tmux.

import { spawn, spawnSync } from "node:child_process";
import { watch } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import electron from "electron";
import { createServer } from "vite";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const frozen = !!process.env.TETHYS_NO_HMR;

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: root, stdio: "inherit" });
  return r.status === 0;
}

// By binary, not package: the three names are what the daemon relies on.
const buildRust = () =>
  run("cargo", [
    "build",
    "--bin",
    "tethysd",
    "--bin",
    "tethys-hook",
    "--bin",
    "tethys-mcp",
  ]);
const buildShell = () => run("node", ["electron/build.mjs"]);

function targetDir() {
  const r = spawnSync(
    "cargo",
    ["metadata", "--format-version", "1", "--no-deps"],
    { cwd: root, encoding: "utf8", maxBuffer: 64 << 20 },
  );
  return JSON.parse(r.stdout).target_directory;
}

if (!buildRust() || !buildShell()) process.exit(1);

const vite = await createServer({ root });
await vite.listen();
const devUrl = vite.resolvedUrls.local[0];

let app = null;
let restarting = false;

function launch() {
  app = spawn(electron, [root], {
    cwd: root,
    stdio: "inherit",
    env: {
      ...process.env,
      TETHYS_DEV_URL: devUrl,
      TETHYS_DAEMON: path.join(targetDir(), "debug", "tethysd"),
    },
  });
  app.on("exit", () => {
    app = null;
    if (restarting) return;
    vite.close().finally(() => process.exit(0));
  });
}

const stop = () =>
  new Promise((resolve) => {
    if (!app) return resolve();
    app.once("exit", resolve);
    app.kill("SIGTERM");
  });

launch();

if (!frozen) {
  let pending = new Set();
  let timer = null;
  const changed = (dir) => (_event, file) => {
    if (!file || file.includes("target/")) return;
    pending.add(dir);
    clearTimeout(timer);
    timer = setTimeout(restart, 300);
  };
  async function restart() {
    const dirs = pending;
    pending = new Set();
    restarting = true;
    await stop();
    const ok =
      (!dirs.has("rust") || buildRust()) &&
      (!dirs.has("shell") || buildShell());
    restarting = false;
    if (ok) launch();
    else console.error("build failed; fix it and save again");
  }
  for (const dir of ["src-tauri", "crates"]) {
    watch(path.join(root, dir), { recursive: true }, changed("rust"));
  }
  watch(path.join(root, "electron"), { recursive: true }, changed("shell"));
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, async () => {
    restarting = true;
    await stop();
    await vite.close();
    process.exit(0);
  });
}
