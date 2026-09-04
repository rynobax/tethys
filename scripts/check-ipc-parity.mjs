#!/usr/bin/env node
/**
 * Assert the frontend's IPC tables match what Rust actually exposes.
 *
 * The two sides are hand-mirrored — there is no ts-rs or specta here, and for
 * a one-person tool that is the right call. What it costs is drift, and drift
 * in this direction is silent: `invoke` takes a string, so a renamed command
 * fails at runtime, in a `.catch` that may only `console.error`.
 *
 * This is far cheaper than code generation and catches the class of bug that
 * actually happened: `workspace:reordered` was emitted, listened to by nobody,
 * and documented in a comment as driving a refresh it did not drive.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const RUST_DIR = "src-tauri/src";

function rustFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory()
      ? rustFiles(join(dir, e.name))
      : e.name.endsWith(".rs")
        ? [join(dir, e.name)]
        : [],
  );
}

const rustSource = rustFiles(RUST_DIR)
  .map((f) => readFileSync(f, "utf8"))
  .join("\n");

// ── commands ───────────────────────────────────────────────────────────────

const registered = new Set(
  // Command fns are snake_case; `commands::ClaudeBin` and friends are types.
  [...readFileSync(join(RUST_DIR, "lib.rs"), "utf8").matchAll(/commands::([a-z]\w*)/g)].map(
    (m) => m[1],
  ),
);

const commandsTs = readFileSync("src/ipc/commands.ts", "utf8");
const referenced = new Set(
  [...commandsTs.matchAll(/invoke<[^>]*>\("(\w+)"|invoke\("(\w+)"|command: "(\w+)"/g)].map(
    (m) => m[1] ?? m[2] ?? m[3],
  ),
);

// ── events ─────────────────────────────────────────────────────────────────

const emitted = new Set(
  [...rustSource.matchAll(/"((?:workspace|session|script|github|system_status|theme|pending_permissions|artifact):\w+)"/g)].map(
    (m) => m[1],
  ),
);

const eventsTs = readFileSync("src/ipc/events.ts", "utf8");
const tableBody = eventsTs.slice(
  eventsTs.indexOf("export interface AppEvents {"),
  eventsTs.indexOf("export type AppEventName"),
);
const declared = new Set([...tableBody.matchAll(/"([\w_]+:[\w_]+)":/g)].map((m) => m[1]));

// ── report ─────────────────────────────────────────────────────────────────

const problems = [];

for (const name of referenced) {
  if (!registered.has(name)) {
    problems.push(`command "${name}" is called from the frontend but not registered in lib.rs`);
  }
}
for (const name of declared) {
  if (!emitted.has(name)) {
    problems.push(`event "${name}" is declared in ipc/events.ts but never emitted by Rust`);
  }
}
for (const name of emitted) {
  if (!declared.has(name)) {
    problems.push(`event "${name}" is emitted by Rust but missing from ipc/events.ts`);
  }
}

// Unused commands are reported separately: not an error (a command may be
// deliberately backend-only), but worth seeing.
const unusedCommands = [...registered].filter((c) => !referenced.has(c)).sort();

if (problems.length > 0) {
  console.error("IPC parity check failed:\n");
  for (const p of problems) console.error(`  ✗ ${p}`);
  console.error("");
  process.exit(1);
}

console.log(
  `IPC parity ok — ${referenced.size}/${registered.size} commands wrapped, ${declared.size} events declared.`,
);
if (unusedCommands.length > 0) {
  console.log(`  no frontend caller: ${unusedCommands.join(", ")}`);
}
