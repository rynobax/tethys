/**
 * Fun mode without Tauri: `pnpm vite --port 5199`, then open /fun-preview.html.
 * Every mood on screen at once, with real designs, for iterating on the look.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { StrictMode, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type { GithubPrStatus, SessionInfo, Workspace } from "../types";
import FunWorld from "./FunWorld";
import "../App.css";

const DESIGNS: Record<string, unknown> = {
  flaky: {
    name: "Jelly Bean",
    tagline: "chasing the cookie race, one wobble at a time",
    body: "blob",
    body_color: "#7FE5C0",
    accent_color: "#FF7AA2",
    eyes: "wide",
    hat: "beanie",
    accessory: "none",
    work: "typing",
    prop: {
      label: "session cookie",
      parts: [
        {
          shape: "cylinder",
          color: "#E8A045",
          position: [0, -0.15, 0],
          scale: [0.8, 0.15, 0.8],
          rotation: [0, 0, 0],
        },
        {
          shape: "sphere",
          color: "#6B2D5C",
          position: [-0.2, -0.05, 0.1],
          scale: [0.14, 0.14, 0.14],
          rotation: [0, 0, 0],
        },
        {
          shape: "sphere",
          color: "#6B2D5C",
          position: [0.15, -0.05, -0.1],
          scale: [0.14, 0.14, 0.14],
          rotation: [0, 0, 0],
        },
        {
          shape: "sphere",
          color: "#6B2D5C",
          position: [0.1, -0.05, 0.2],
          scale: [0.14, 0.14, 0.14],
          rotation: [0, 0, 0],
        },
      ],
    },
  },
  migration: {
    name: "Nora Malize",
    tagline: "unpacking blobs into tidy columns, one row at a time",
    body: "round",
    body_color: "#b8a1ff",
    accent_color: "#ffd166",
    eyes: "dots",
    hat: "hardhat",
    accessory: "backpack",
    work: "sweeping",
    prop: {
      label: "migration crate",
      parts: [
        {
          shape: "box",
          color: "#5b8def",
          position: [0, -0.2, 0],
          scale: [0.8, 0.6, 0.8],
          rotation: [0, 0, 0],
        },
        {
          shape: "box",
          color: "#ffd166",
          position: [0, 0.12, 0],
          scale: [0.86, 0.08, 0.86],
          rotation: [0, 0, 0],
        },
        {
          shape: "cone",
          color: "#ffd166",
          position: [0, 0.36, 0],
          scale: [0.3, 0.3, 0.3],
          rotation: [0, 0, 0],
        },
      ],
    },
  },
  perf: {
    name: "Batchling",
    tagline: "one trip to the db instead of 1,400 tiny ones",
    body: "round",
    body_color: "#7FD8BE",
    accent_color: "#FF8FA3",
    eyes: "wide",
    hat: "headphones",
    accessory: "backpack",
    work: "typing",
    prop: {
      label: "stack of crates carried in one trip",
      parts: [
        {
          shape: "box",
          color: "#FFB347",
          position: [0, -0.35, 0],
          scale: [0.9, 0.3, 0.9],
          rotation: [0, 0, 0],
        },
        {
          shape: "box",
          color: "#FFD166",
          position: [0, -0.05, 0],
          scale: [0.7, 0.3, 0.7],
          rotation: [0, 0, 0],
        },
        {
          shape: "box",
          color: "#F4A261",
          position: [0, 0.25, 0],
          scale: [0.5, 0.3, 0.5],
          rotation: [0, 0, 0],
        },
      ],
    },
  },
  security: {
    name: "Sir Locksalot",
    tagline: "guarding the bucket, one ../ at a time",
    body: "tall",
    body_color: "#7fd8c0",
    accent_color: "#ff7aa2",
    eyes: "visor",
    hat: "none",
    accessory: "cape",
    work: "typing",
    prop: {
      label: "padlock on the upload gate",
      parts: [
        {
          shape: "torus",
          color: "#ffd166",
          position: [0, 0.2, 0],
          scale: [0.5, 0.5, 0.15],
          rotation: [0, 0, 0],
        },
        {
          shape: "box",
          color: "#ffd166",
          position: [0, -0.12, 0],
          scale: [0.7, 0.5, 0.25],
          rotation: [0, 0, 0],
        },
      ],
    },
  },
  orchestrate: {
    name: "Tidy Tally",
    tagline: "sweeping 9 lint rules into their own workspaces",
    body: "round",
    body_color: "#8fe3c4",
    accent_color: "#ff8fab",
    eyes: "dots",
    hat: "beanie",
    accessory: "backpack",
    work: "sweeping",
    prop: {
      label: "lint broom",
      parts: [
        {
          shape: "cylinder",
          color: "#c9a66b",
          position: [0, 0.05, 0],
          scale: [0.08, 0.9, 0.08],
          rotation: [0, 0, 20],
        },
        {
          shape: "box",
          color: "#ffd166",
          position: [0.12, -0.4, 0],
          scale: [0.45, 0.22, 0.14],
          rotation: [0, 0, 20],
        },
      ],
    },
  },
};

const running = { running: true } as SessionInfo;

interface Sample {
  id: string;
  branch: string;
  design?: string;
  mood:
    | "needs_you"
    | "working"
    | "idle"
    | "asleep"
    | "queued"
    | "creating"
    | "failed"
    | "shipping"
    | "shipped";
  parent?: string;
  progress?: { done: number; total: number };
  /** For a Shipping sample: approved and green, so it's waiting on Ryan. */
  approved?: boolean;
}

const SAMPLES: Sample[] = [
  { id: "a", branch: "fix-flaky-login", design: "flaky", mood: "needs_you" },
  {
    id: "b",
    branch: "reports-normalize",
    design: "migration",
    mood: "working",
  },
  { id: "c", branch: "worklist-n-plus-1", design: "perf", mood: "working" },
  {
    id: "d",
    branch: "upload-traversal",
    design: "security",
    mood: "needs_you",
  },
  {
    id: "e",
    branch: "lint-rollout",
    design: "orchestrate",
    mood: "idle",
    progress: { done: 3, total: 9 },
  },
  { id: "e1", branch: "lint-no-floating", mood: "idle", parent: "e" },
  { id: "e2", branch: "lint-exhaustive-deps", mood: "working", parent: "e" },
  { id: "e3", branch: "lint-no-shadow", mood: "idle", parent: "e" },
  { id: "f", branch: "old-experiment", mood: "asleep" },
  { id: "f2", branch: "spike-pdf-export", design: "perf", mood: "idle" },
  { id: "f3", branch: "try-new-linter", design: "security", mood: "idle" },
  { id: "f4", branch: "dead-end-idea", mood: "asleep" },
  { id: "k", branch: "metadata-prefetch-cache", mood: "needs_you" },
  { id: "l", branch: "acr-score-form-insert-via-intent", mood: "needs_you" },
  {
    id: "m",
    branch: "reporter-settings-menu",
    design: "migration",
    mood: "needs_you",
  },
  { id: "g", branch: "new-thing", mood: "creating" },
  { id: "h", branch: "next-in-line", mood: "queued" },
  { id: "i", branch: "ship-the-thing", design: "perf", mood: "shipping" },
  { id: "j", branch: "already-merged", design: "flaky", mood: "shipped" },
  { id: "n", branch: "hutch", design: "security", mood: "shipping" },
  { id: "o", branch: "metadata-template-id", mood: "shipping" },
  {
    id: "p",
    branch: "approved-and-green",
    design: "migration",
    mood: "shipping",
    approved: true,
  },
];

const prNumber = (s: Sample) => 7000 + s.id.charCodeAt(0);

const workspace = (s: Sample): Workspace => ({
  id: s.id,
  branch: s.branch,
  created_at: "",
  repo_links:
    s.mood === "shipping"
      ? [
          {
            repo_key: "nl-frontend",
            worktree_path: "",
            setup_script_ran_at: null,
            dismissed: [],
            prs: [
              {
                number: prNumber(s),
                tracked_at: "",
                status: {
                  state: "open",
                  checks: "success",
                  has_merge_conflicts: false,
                  review_decision: s.approved ? "approved" : "review_required",
                  review_requested: true,
                  merge_queue: null,
                } as GithubPrStatus,
              },
            ],
          },
        ]
      : [],
  session: null,
  agent: "claude",
  agent_binary: null,
  deleted_at: null,
  folder: null,
  status: {
    kind:
      s.mood === "queued"
        ? "queued"
        : s.mood === "creating"
          ? "creating"
          : s.mood === "failed"
            ? "creation_failed"
            : "ready",
  } as Workspace["status"],
  notes: "",
  parent: s.parent ?? null,
  shipment:
    s.mood === "shipping"
      ? {
          kind: "shipping",
          prs: [{ repo_key: "nl-frontend", number: prNumber(s) }],
          since: "",
        }
      : s.mood === "shipped"
        ? { kind: "shipped", at: "" }
        : null,
  progress: s.progress ?? null,
});

mockIPC((cmd) => {
  if (cmd === "list_avatars") {
    return Object.fromEntries(
      SAMPLES.filter((s) => s.design).map((s) => [
        s.id,
        { spec: DESIGNS[s.design!], generated_at: "" },
      ]),
    );
  }
  return null;
});

function Preview() {
  const panelRef = useRef<HTMLElement>(null);
  const [selected, setSelected] = useState<string | null>("b");
  const [samples, setSamples] = useState(SAMPLES);
  // `?demo`: approved-and-green is merged after 3s, already-merged deleted
  // after 5s, to watch the cork pop and the boat leave.
  useEffect(() => {
    if (!location.search.includes("demo")) return;
    const merge = setTimeout(
      () =>
        setSamples((ss) =>
          ss.map((s) => (s.id === "p" ? { ...s, mood: "shipped" } : s)),
        ),
      3000,
    );
    const remove = setTimeout(
      () => setSamples((ss) => ss.filter((s) => s.id !== "j")),
      5000,
    );
    return () => {
      clearTimeout(merge);
      clearTimeout(remove);
    };
  }, []);
  const bySample = new Map(samples.map((s) => [s.id, s]));
  return (
    <div className="app fun">
      <FunWorld
        workspaces={samples.map(workspace)}
        sessions={
          new Map(
            samples.map((s) => [s.id, s.mood === "asleep" ? null : running]),
          )
        }
        needsTurn={(w) => bySample.get(w.id)?.mood === "needs_you"}
        working={(w) => bySample.get(w.id)?.mood === "working"}
        selectedId={selected}
        menuActions={{
          onDelete: (w) => setSamples((ss) => ss.filter((s) => s.id !== w.id)),
          onClearTurn: (w) => console.log("clear", w.branch),
          onStartShipping: (w) => console.log("ship", w.branch),
          onStopShipping: (w) => console.log("stop shipping", w.branch),
        }}
        onSelect={setSelected}
        panelRef={panelRef}
      />
      {/* The real detail pane's skeleton, so the pane sizes as it does live. */}
      <main className="detail" ref={panelRef}>
        <div className="workspace-detail">
          <div className="workspace-main">
            <div style={{ padding: "8px 12px" }}>{selected}</div>
            <div className="session-pane">
              <div
                className="session-terminal"
                style={{ background: "rgba(127, 127, 127, 0.15)" }}
              />
            </div>
          </div>
          <button type="button" className="side-rail" />
        </div>
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Preview />
  </StrictMode>,
);
