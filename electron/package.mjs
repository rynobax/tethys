#!/usr/bin/env node
// Packages a release build as `target/app/Tethys.app`, with `tethysd`,
// `tethys-hook` and `tethys-mcp` beside the Electron executable in
// `Contents/MacOS/`, since the daemon finds the other two as its siblings.

import { spawnSync } from "node:child_process";
import { copyFileSync, readFileSync, renameSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { packager } from "@electron/packager";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { cwd: root, stdio: "inherit", ...opts });
  if (r.status !== 0) {
    console.error(`${cmd} ${args.join(" ")} failed`);
    process.exit(1);
  }
  return r;
}

const BINARIES = ["tethysd", "tethys-hook", "tethys-mcp"];

run("cargo", ["build", "--release", ...BINARIES.flatMap((b) => ["--bin", b])]);
run("pnpm", ["build"]);

const target = JSON.parse(
  run("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    stdio: ["ignore", "pipe", "inherit"],
    encoding: "utf8",
    maxBuffer: 64 << 20,
  }).stdout,
).target_directory;

const out = path.join(target, "app");
const electronVersion = JSON.parse(
  readFileSync(path.join(root, "node_modules/electron/package.json"), "utf8"),
).version;

// Everything the app runs is bundled into `dist/`; no node_modules ship.
const keep = (p) =>
  p === "" ||
  p === "/package.json" ||
  p === "/dist" ||
  p.startsWith("/dist/renderer") ||
  p.startsWith("/dist/electron");

const [built] = await packager({
  dir: root,
  out,
  overwrite: true,
  platform: "darwin",
  arch: process.arch,
  electronVersion,
  name: "Tethys",
  appBundleId: "app.tethys.dev",
  icon: path.join(root, "src-tauri/icons/icon.icns"),
  asar: true,
  prune: false,
  ignore: (p) => !keep(p),
});

const app = path.join(out, "Tethys.app");
rmSync(app, { recursive: true, force: true });
renameSync(path.join(built, "Tethys.app"), app);
rmSync(built, { recursive: true, force: true });

for (const bin of BINARIES) {
  copyFileSync(
    path.join(target, "release", bin),
    path.join(app, "Contents/MacOS", bin),
  );
}
// Apple Silicon won't launch a modified bundle without at least an ad-hoc
// signature.
run("codesign", ["--force", "--deep", "--sign", "-", app]);

console.log(`Packaged ${app}`);
