/**
 * Fun mode without Tauri: `pnpm vite --port 5199`, then open /fun-preview.html.
 * Every mood on screen at once, for iterating on the look; `?gallery` for
 * every animal side by side instead.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { StrictMode, useCallback, useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import type { Folder, GithubPrStatus, Workspace } from "../types";
import FunWorld, { type ManualClock } from "./FunWorld";
import Gallery from "./Gallery";
import "../App.css";

interface Sample {
  id: string;
  branch: string;
  mood:
    | "needs_you"
    | "working"
    | "idle"
    | "queued"
    | "creating"
    | "failed"
    | "shipping"
    | "shipped";
  parent?: string;
  folder?: string;
  progress?: { done: number; total: number };
  /** For a Shipping sample: approved and green, so it's waiting on Ryan. */
  approved?: boolean;
}

const SAMPLES: Sample[] = [
  { id: "a", branch: "fix-flaky-login", mood: "needs_you" },
  {
    id: "b",
    branch: "reports-normalize",
    mood: "working",
  },
  {
    id: "c",
    branch: "worklist-n-plus-1",
    mood: "working",
    progress: { done: 5, total: 5 },
  },
  {
    id: "d",
    branch: "upload-traversal",
    mood: "needs_you",
  },
  {
    id: "e",
    branch: "lint-rollout",
    folder: "lint",
    mood: "idle",
    progress: { done: 3, total: 9 },
  },
  {
    id: "e1",
    branch: "lint-no-floating",
    mood: "idle",
    parent: "e",
    folder: "lint",
  },
  {
    id: "e2",
    branch: "lint-exhaustive-deps",
    mood: "working",
    parent: "e",
    folder: "lint",
  },
  {
    id: "e3",
    branch: "lint-no-shadow",
    mood: "idle",
    parent: "e",
    folder: "lint",
  },
  { id: "f", branch: "old-experiment", mood: "idle", folder: "spikes" },
  { id: "f2", branch: "spike-pdf-export", mood: "idle", folder: "spikes" },
  { id: "f3", branch: "try-new-linter", mood: "idle" },
  { id: "f4", branch: "dead-end-idea", mood: "idle" },
  {
    id: "k",
    branch: "metadata-prefetch-cache",
    mood: "needs_you",
    folder: "meta",
  },
  { id: "l", branch: "acr-score-form-insert-via-intent", mood: "needs_you" },
  {
    id: "m",
    branch: "reporter-settings-menu",
    mood: "needs_you",
  },
  { id: "g", branch: "new-thing", mood: "creating" },
  { id: "h", branch: "next-in-line", mood: "queued" },
  { id: "i", branch: "ship-the-thing", mood: "shipping" },
  { id: "j", branch: "already-merged", mood: "shipped" },
  { id: "n", branch: "hutch", mood: "shipping" },
  { id: "o", branch: "metadata-template-id", mood: "shipping" },
  {
    id: "p",
    branch: "approved-and-green",
    mood: "shipping",
    approved: true,
  },
];

const FOLDERS: Folder[] = [
  { id: "lint", name: "Lint rollout", collapsed: false },
  { id: "spikes", name: "Spikes", collapsed: false },
  { id: "meta", name: "Metadata", collapsed: true },
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
  folder: s.folder ?? null,
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

mockIPC(() => null);

function Preview() {
  const panelRef = useRef<HTMLElement>(null);
  const [selected, setSelected] = useState<string | null>("b");
  const [samples, setSamples] = useState(SAMPLES);
  const [folders, setFolders] = useState(FOLDERS);
  const fold = (id: string, collapsed: boolean) =>
    setFolders((fs) => fs.map((f) => (f.id === id ? { ...f, collapsed } : f)));
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
  // `?shot`: time stands still, and `window.__fun` moves it and changes the
  // samples, for `scripts/fun-shot.mjs`. Changes are flushed straight away so
  // the next step already sees them.
  const shot = location.search.includes("shot");
  const onManualClock = useCallback((clock: ManualClock) => {
    const set = (fn: (ss: Sample[]) => Sample[]) =>
      flushSync(() => setSamples(fn));
    Object.assign(window, {
      __fun: {
        step: clock.step,
        sim: clock.sim,
        mood: (id: string, mood: Sample["mood"]) =>
          set((ss) => ss.map((s) => (s.id === id ? { ...s, mood } : s))),
        remove: (id: string) => set((ss) => ss.filter((s) => s.id !== id)),
        progress: (id: string, done: number, total: number) =>
          set((ss) =>
            ss.map((s) =>
              s.id === id ? { ...s, progress: { done, total } } : s,
            ),
          ),
        select: (id: string | null) => flushSync(() => setSelected(id)),
        fold: (id: string, collapsed: boolean) =>
          flushSync(() =>
            setFolders((fs) =>
              fs.map((f) => (f.id === id ? { ...f, collapsed } : f)),
            ),
          ),
        ids: () => SAMPLES.map((s) => `${s.id} ${s.branch} ${s.mood}`),
      },
    });
  }, []);
  return (
    <div className="app fun">
      <FunWorld
        workspaces={samples.map(workspace)}
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
        folders={folders}
        onSetFolderCollapsed={fold}
        panelRef={panelRef}
        onManualClock={shot ? onManualClock : undefined}
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
    {location.search.includes("gallery") ? <Gallery /> : <Preview />}
  </StrictMode>,
);
