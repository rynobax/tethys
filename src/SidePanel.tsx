import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import * as api from "./ipc/commands";
import { useAppEvent } from "./ipc/events";
import { useTheme } from "./theme";
import type { Artifact, Workspace } from "./types";
import { useHorizontalScroll } from "./useHorizontalScroll";

/** JSON map of workspace id → panel width in px. A workspace with no entry
 *  opens at half the detail pane (see `DEFAULT_WIDTH`). */
const WIDTHS_KEY = "tethys.sidePanel.widths";
/** JSON map of workspace id → collapsed. A workspace with no entry starts
 *  collapsed: the terminal is the point until something asks for the panel. */
const COLLAPSED_KEY = "tethys.sidePanel.collapsedByWorkspace";
/** The width until you drag it: an even split with the terminal. A CSS
 *  percentage rather than a measured pixel count, so it stays an even split
 *  as the window resizes and needs no layout pass to compute. */
const DEFAULT_WIDTH = "50%";
const MIN_WIDTH = 280;

/** Read one of the per-workspace maps out of `localStorage`, keeping only the
 *  entries `valid` vouches for. Missing or unparseable starts fresh: nothing
 *  in here is worth recovering. */
function loadMap<T>(
  key: string,
  valid: (v: unknown) => v is T,
): Record<string, T> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "");
    if (parsed && typeof parsed === "object") {
      return Object.fromEntries(
        Object.entries(parsed as Record<string, unknown>).filter(
          (e): e is [string, T] => valid(e[1]),
        ),
      );
    }
  } catch {
    // fall through
  }
  return {};
}

const isPanelWidth = (v: unknown): v is number =>
  typeof v === "number" && v >= MIN_WIDTH;
const isBoolean = (v: unknown): v is boolean => typeof v === "boolean";
/** How far the PR webview's left edge pulls back from the panel's while the
 *  panel is being resized. At a fast drag the native view trails the DOM by a
 *  frame or two — tens of pixels — and this keeps the cursor off it. */
const DRAG_GUARD_PX = 64;

// "notes", an artifact id, or a `pr:<url>` id for an embedded PR tab.
type TabId = "notes" | string;

interface PrTab {
  /** `pr:<url>` — unique per PR, and distinct from any artifact id. */
  id: string;
  number: number;
  url: string;
  repoKey: string;
}

interface Props {
  workspace: Workspace;
  /** Live notes text — App's draft when there is one, else the persisted
   *  `workspace.notes`. */
  notes: string;
  onNotesChange: (notes: string) => void;
}

/**
 * The Side Panel: a workspace's Notes and its Artifacts, one tab each, on the
 * right of the detail pane.
 *
 * Collapses to a thin rail; the rail is the whole affordance for expanding it
 * again. Collapsed state and width are both per workspace — one that's mostly
 * a PR page wants half the screen open, one that's just a terminal wants the
 * rail — and both live in `localStorage` as id-keyed maps. A workspace you've
 * never touched starts collapsed at an even split. The one thing that
 * overrides your choice is a fresh artifact for the workspace you're looking
 * at: that expands the panel and selects the new tab, because a `/show-me`
 * turn is one where you want the screen taken.
 */
export function SidePanel({ workspace, notes, onNotesChange }: Props) {
  // True for the length of a resize drag; the PR webview keeps a guard strip
  // clear of the cursor meanwhile (see `PrView`).
  const [resizing, setResizing] = useState(false);
  const [collapsedMap, setCollapsedMap] = useState(() =>
    loadMap(COLLAPSED_KEY, isBoolean),
  );
  const collapsed = collapsedMap[workspace.id] ?? true;
  const persistCollapsed = (value: boolean) => {
    setCollapsedMap((prev) => {
      const next = { ...prev, [workspace.id]: value };
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next));
      return next;
    });
  };
  const [widths, setWidths] = useState(() =>
    loadMap(WIDTHS_KEY, isPanelWidth),
  );
  // Pixels once this workspace's panel has been dragged, else the default split.
  const width: number | string = widths[workspace.id] ?? DEFAULT_WIDTH;
  const setWorkspaceWidth = (w: number) => {
    setWidths((prev) => {
      const next = { ...prev, [workspace.id]: w };
      localStorage.setItem(WIDTHS_KEY, JSON.stringify(next));
      return next;
    });
  };
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  // Remembered per workspace so switching back paints the tab you left.
  const [selectedByWorkspace, setSelectedByWorkspace] = useState<
    Map<string, TabId>
  >(new Map());

  const select = useCallback(
    (tab: TabId) => {
      setSelectedByWorkspace((prev) => {
        const next = new Map(prev);
        next.set(workspace.id, tab);
        return next;
      });
    },
    [workspace.id],
  );

  const refresh = useCallback(() => {
    api
      .listArtifacts(workspace.id)
      .then(setArtifacts)
      .catch((e) => console.error("list_artifacts failed:", e));
  }, [workspace.id]);

  useEffect(() => {
    setArtifacts([]);
    refresh();
  }, [refresh]);

  useAppEvent("artifact:changed", (payload) => {
    if (payload.workspace_id !== workspace.id) return;
    refresh();
    if (payload.artifact_id) {
      select(payload.artifact_id);
      persistCollapsed(false);
    }
  });

  // One tab per linked PR that has been fetched at least once (a `null`
  // status has no URL to load). Deduped by URL so a PR tracked twice is one
  // tab, and labelled by repo so two repos' `#123`s are told apart.
  const prTabs = useMemo<PrTab[]>(() => {
    const seen = new Set<string>();
    const tabs: PrTab[] = [];
    for (const link of workspace.repo_links) {
      for (const pr of link.prs) {
        const url = pr.status?.url;
        if (!url || seen.has(url)) continue;
        seen.add(url);
        tabs.push({
          id: `pr:${url}`,
          number: pr.number,
          url,
          repoKey: link.repo_key,
        });
      }
    }
    return tabs;
  }, [workspace.repo_links]);

  // Effective tab: the remembered pick when it still exists, else the newest
  // artifact (last in the list), else Notes. A PR tab is only ever reached by
  // an explicit click, never auto-selected.
  const remembered = selectedByWorkspace.get(workspace.id);
  const selected: TabId =
    remembered !== undefined &&
    (remembered === "notes" ||
      artifacts.some((a) => a.id === remembered) ||
      prTabs.some((t) => t.id === remembered))
      ? remembered
      : (artifacts[artifacts.length - 1]?.id ?? "notes");
  const selectedArtifact = artifacts.find((a) => a.id === selected) ?? null;
  const selectedPr = prTabs.find((t) => t.id === selected) ?? null;

  // The tab strip scrolls sideways once it fills, and the active tab is kept
  // in view — a fresh artifact selects itself, and it's appended at the far
  // end, exactly where an overflowing strip has scrolled away from.
  const strip = useRef<HTMLDivElement | null>(null);
  const wheelRef = useHorizontalScroll<HTMLDivElement>();
  const tabsRef = useCallback(
    (el: HTMLDivElement | null) => {
      strip.current = el;
      wheelRef(el);
    },
    [wheelRef],
  );
  useEffect(() => {
    strip.current
      ?.querySelector(".side-tab.active")
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [selected, collapsed]);

  const dismiss = (id: string) => {
    // Pick the neighbour before the list shrinks: right, else left, else Notes.
    if (selected === id) {
      const i = artifacts.findIndex((a) => a.id === id);
      const next = artifacts[i + 1] ?? artifacts[i - 1];
      select(next ? next.id : "notes");
    }
    setArtifacts((prev) => prev.filter((a) => a.id !== id));
    api
      .dismissArtifact(workspace.id, id)
      .catch((e) => console.error("dismiss_artifact failed:", e));
  };

  if (collapsed) {
    return (
      <button
        type="button"
        className="side-rail"
        onClick={() => persistCollapsed(false)}
        title="Expand side panel"
      >
        <span className="side-rail-label">
          Notes
          {notes.trim() && <span className="side-rail-dot" />}
        </span>
        {artifacts.length > 0 && (
          <span className="side-rail-count">{artifacts.length}</span>
        )}
      </button>
    );
  }

  return (
    <aside className="side-panel" style={{ width }}>
      <ResizeHandle
        onResize={setWorkspaceWidth}
        onDragStart={() => setResizing(true)}
        onDragEnd={() => setResizing(false)}
      />
      <div className="side-tabs" role="tablist" ref={tabsRef}>
        <button
          type="button"
          role="tab"
          className={`side-tab ${selected === "notes" ? "active" : ""}`}
          onClick={() => select("notes")}
        >
          Notes{notes.trim() && <span className="side-rail-dot" />}
        </button>
        {artifacts.map((a) => (
          <div
            key={a.id}
            role="tab"
            className={`side-tab artifact ${selected === a.id ? "active" : ""}`}
            onClick={() => select(a.id)}
            title={a.kind === "page" ? a.path : a.label}
          >
            <span className="side-tab-glyph">
              {a.kind === "diagram" ? "◇" : "▤"}
            </span>
            <span className="side-tab-label">{a.label}</span>
            <button
              type="button"
              className="side-tab-close"
              onClick={(e) => {
                e.stopPropagation();
                dismiss(a.id);
              }}
              title="Close"
            >
              ✕
            </button>
          </div>
        ))}
        {prTabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            className={`side-tab pr ${selected === t.id ? "active" : ""}`}
            onClick={() => select(t.id)}
            title={`${t.repoKey} #${t.number}`}
          >
            <span className="side-tab-glyph">⑂</span>
            <span className="side-tab-label">#{t.number}</span>
          </button>
        ))}
        <button
          type="button"
          className="side-collapse"
          onClick={() => persistCollapsed(true)}
          title="Collapse side panel"
        >
          »
        </button>
      </div>
      <div className="side-body">
        {selectedPr ? (
          <PrView
            url={selectedPr.url}
            number={selectedPr.number}
            repoKey={selectedPr.repoKey}
            resizing={resizing}
          />
        ) : selectedArtifact === null ? (
          <NotesTab
            key={workspace.id}
            workspaceId={workspace.id}
            notes={notes}
            onNotesChange={onNotesChange}
          />
        ) : selectedArtifact.kind === "diagram" ? (
          <DiagramView
            key={selectedArtifact.id}
            source={selectedArtifact.source}
          />
        ) : (
          <PageView workspaceId={workspace.id} artifact={selectedArtifact} />
        )}
      </div>
    </aside>
  );
}

/** Drag the panel's left edge to resize it. */
/**
 * The drag handle on the panel's left edge.
 *
 * Uses pointer capture so every move and the final release come to the handle
 * itself, wherever the cursor has wandered — over the terminal, off the window.
 * The one thing capture can't cross is the native PR webview, which sits above
 * the whole DOM and swallows events at the OS level: a drag that ended over it
 * never saw its mouseup and kept resizing forever. Two defences: the drag is
 * bracketed by `onDragStart`/`onDragEnd` so the panel can keep that webview
 * clear of the cursor, and a move that arrives with no button held means the
 * release happened where we couldn't see it, so the drag ends there. A window
 * blur (Cmd-Tab mid-drag) ends it too, since no release is coming.
 */
function ResizeHandle({
  onResize,
  onDragStart,
  onDragEnd,
}: {
  onResize: (width: number) => void;
  onDragStart: () => void;
  onDragEnd: () => void;
}) {
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    const startX = e.clientX;
    // Measured, not passed in: until it's been dragged the panel's width is a
    // percentage, and the drag has to start from the pixels that resolves to.
    const startWidth =
      handle.parentElement?.getBoundingClientRect().width ?? MIN_WIDTH;
    const max = Math.floor(window.innerWidth * 0.7);

    const onMove = (ev: PointerEvent) => {
      if ((ev.buttons & 1) === 0) {
        finish();
        return;
      }
      const next = startWidth + (startX - ev.clientX);
      onResize(Math.max(MIN_WIDTH, Math.min(max, next)));
    };
    const finish = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", finish);
      handle.removeEventListener("pointercancel", finish);
      window.removeEventListener("blur", finish);
      if (handle.hasPointerCapture(e.pointerId)) {
        handle.releasePointerCapture(e.pointerId);
      }
      document.body.style.cursor = "";
      onDragEnd();
    };

    onDragStart();
    document.body.style.cursor = "col-resize";
    handle.setPointerCapture(e.pointerId);
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", finish);
    handle.addEventListener("pointercancel", finish);
    window.addEventListener("blur", finish);
  };
  return <div className="side-resize" onPointerDown={onPointerDown} />;
}

/**
 * Freeform notes editor. Edits are debounced to `set_workspace_notes` and
 * flushed on unmount so nothing is lost when switching tabs or workspaces.
 * Keyed by workspace id at the call site so each workspace gets a fresh
 * editor; the text itself lives in App's `noteDrafts` so it survives that
 * remount.
 */
function NotesTab({
  workspaceId,
  notes,
  onNotesChange,
}: {
  workspaceId: string;
  notes: string;
  onNotesChange: (notes: string) => void;
}) {
  const saveTimer = useRef<number | null>(null);
  // Latest unsaved value, or null once it's been persisted. Lets the flush on
  // unmount write the final keystrokes the debounce hasn't sent yet.
  const pending = useRef<string | null>(null);

  const save = useCallback(
    (notes: string) => {
      pending.current = null;
      api.setWorkspaceNotes(workspaceId, notes).catch(() => {
        // Best-effort persistence; the text stays in the editor regardless.
      });
    },
    [workspaceId],
  );

  const flush = useCallback(() => {
    if (saveTimer.current !== null) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    if (pending.current !== null) save(pending.current);
  }, [save]);

  useEffect(() => flush, [flush]);

  const onChange = (value: string) => {
    onNotesChange(value);
    pending.current = value;
    if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = null;
      save(value);
    }, 500);
  };

  return (
    <textarea
      className="notes-textarea"
      value={notes}
      placeholder="Jot down anything about this workspace…"
      onChange={(e) => onChange(e.target.value)}
    />
  );
}

/**
 * A mermaid diagram, rendered fit-to-width and left to scroll vertically.
 * `mermaid` is a couple of megabytes, so it's imported on first use rather
 * than at boot. A diagram that doesn't parse — Claude emits those fairly
 * often — shows its source and the parser's complaint, which is still more
 * readable than the terminal and tells you what to ask for.
 */
function DiagramView({ source }: { source: string }) {
  const theme = useTheme();
  const dark = theme ? isDark(theme.colors.background) : prefersDark();
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setSvg(null);
    setError(null);
    renderMermaid(source, dark)
      .then((out) => {
        if (!cancelled) setSvg(out);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [source, dark]);

  const copy = () => {
    navigator.clipboard
      .writeText(source)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      })
      .catch((e) => console.error("clipboard write failed:", e));
  };

  return (
    <div className="artifact-view">
      <div className="artifact-toolbar">
        <span className="artifact-toolbar-title">mermaid</span>
        <button type="button" onClick={copy}>
          {copied ? "Copied" : "Copy source"}
        </button>
      </div>
      {svg ? (
        <div
          className="artifact-diagram"
          dangerouslySetInnerHTML={{ __html: svg }}
        />
      ) : error ? (
        <div className="artifact-broken">
          <pre className="artifact-source">{source}</pre>
          <div className="artifact-error">{error}</div>
        </div>
      ) : (
        <div className="artifact-pending">Rendering…</div>
      )}
    </div>
  );
}

let mermaidCounter = 0;

async function renderMermaid(source: string, dark: boolean): Promise<string> {
  const mermaid = (await import("mermaid")).default;
  mermaid.initialize({
    startOnLoad: false,
    theme: dark ? "dark" : "default",
    securityLevel: "strict",
    fontFamily: "ui-sans-serif, system-ui, sans-serif",
  });
  const id = `tethys-mermaid-${mermaidCounter++}`;
  try {
    const { svg } = await mermaid.render(id, source);
    return svg;
  } finally {
    // On a parse error mermaid leaves its scratch element behind.
    document.getElementById(`d${id}`)?.remove();
  }
}

/**
 * An HTML page the session wrote, loaded over the asset protocol so a
 * stylesheet or image beside it resolves too. The iframe is keyed on the
 * artifact's revision, so every re-edit reloads it. Sandboxed without
 * `allow-same-origin`, so whatever the page runs can't reach Tethys's own
 * window.
 */
function PageView({
  workspaceId,
  artifact,
}: {
  workspaceId: string;
  artifact: Artifact & { kind: "page" };
}) {
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="artifact-view">
      <div className="artifact-toolbar">
        <span className="artifact-toolbar-title" title={artifact.path}>
          {artifact.label}
        </span>
        <button
          type="button"
          onClick={() =>
            api
              .openArtifact(workspaceId, artifact.id)
              .catch((e) => setError(String(e)))
          }
        >
          Open in browser
        </button>
      </div>
      {error && <div className="artifact-error">{error}</div>}
      <iframe
        key={artifact.revision}
        className="artifact-page"
        title={artifact.label}
        src={api.convertFileSrc(artifact.path)}
        sandbox="allow-scripts"
      />
    </div>
  );
}

/**
 * The live GitHub PR page for one linked PR. The page can't be shown in an
 * iframe — GitHub forbids being framed — so it renders in a native child
 * webview (`pr_view.rs`) that floats over this component's host `<div>`. This
 * component owns only the geometry and the show/hide lifecycle: it measures
 * the host rect and hands it to Rust, re-measuring whenever the panel or
 * window resizes, and hides the webview when it unmounts (a switch to Notes,
 * an artifact, another workspace, or a collapsed panel).
 *
 * Login lives in the webview's own persistent cookie store, shared across
 * workspaces and restarts — you sign in to GitHub once, inside Tethys.
 */
function PrView({
  url,
  number,
  repoKey,
  resizing,
}: {
  url: string;
  number: number;
  repoKey: string;
  /** True while the panel's edge is being dragged. The webview stays on
   *  screen and follows the host, but with its left edge inset by
   *  `DRAG_GUARD_PX`: it's a native view above the DOM, so if the cursor ever
   *  lands on it the page stops hearing the drag. The webview tracks the host
   *  a frame or so behind, and a quick pull to the right can put the cursor
   *  inside that stale rectangle; the guard strip is where it lands instead. */
  resizing: boolean;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);

  // Position/show the webview to match the host, and keep it matched as the
  // layout changes. Re-runs on `url` so switching PR tabs swaps webviews in
  // place, and on `resizing` so the guard strip appears when a drag starts and
  // closes up the moment it ends.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const inset = resizing ? DRAG_GUARD_PX : 0;
    const sync = () => {
      const r = host.getBoundingClientRect();
      // A zero-size or off-screen host means the panel is mid-collapse or
      // hidden; don't paint a webview into nothing.
      if (r.width - inset < 2 || r.height < 2) {
        api.hidePrView().catch(() => {});
        return;
      }
      api
        .showPrView(url, {
          x: r.left + inset,
          y: r.top,
          width: r.width - inset,
          height: r.height,
        })
        .catch((e) => console.error("show_pr_view failed:", e));
    };
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(host);
    window.addEventListener("resize", sync);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", sync);
    };
  }, [url, resizing]);

  // Hide only when the PR view actually leaves the screen — kept separate from
  // the sync effect so switching between two PR tabs never flashes to hidden.
  useEffect(() => {
    return () => {
      api.hidePrView().catch(() => {});
    };
  }, []);

  return (
    <div className="artifact-view">
      <div className="artifact-toolbar">
        <span
          className="artifact-toolbar-title"
          title={`${repoKey} #${number}`}
        >
          {repoKey} #{number}
        </span>
        <button
          type="button"
          onClick={() =>
            openUrl(url).catch((e) => console.error("openUrl failed:", e))
          }
        >
          Open in browser
        </button>
      </div>
      <div ref={hostRef} className="pr-view-host" />
    </div>
  );
}

function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

function prefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** Relative luminance of a `#rrggbb` colour is below the midpoint. */
function isDark(hex: string): boolean {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(hex);
  if (!m) return prefersDark();
  const [r, g, b] = [m[1], m[2], m[3]].map((h) => parseInt(h, 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5;
}
