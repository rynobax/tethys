import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "./ipc/commands";
import { useAppEvent } from "./ipc/events";
import { useTheme } from "./theme";
import type { Artifact, Workspace } from "./types";

const WIDTH_KEY = "tethys.sidePanel.width";
const COLLAPSED_KEY = "tethys.sidePanel.collapsed";
const DEFAULT_WIDTH = 420;
const MIN_WIDTH = 280;

type TabId = "notes" | string;

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
 * again. Width and collapsed state are UI chrome, so they live in
 * `localStorage` and apply to every workspace — a per-workspace collapse
 * would surprise you on every switch. The one thing that overrides your
 * choice is a fresh artifact for the workspace you're looking at: that
 * expands the panel and selects the new tab, because a `/show-me` turn is one
 * where you want the screen taken.
 */
export function SidePanel({ workspace, notes, onNotesChange }: Props) {
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem(COLLAPSED_KEY) !== "false",
  );
  const [width, setWidth] = useState(() => {
    const stored = Number(localStorage.getItem(WIDTH_KEY));
    return stored >= MIN_WIDTH ? stored : DEFAULT_WIDTH;
  });
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  // Remembered per workspace so switching back paints the tab you left.
  const [selectedByWorkspace, setSelectedByWorkspace] = useState<
    Map<string, TabId>
  >(new Map());

  const persistCollapsed = (value: boolean) => {
    setCollapsed(value);
    localStorage.setItem(COLLAPSED_KEY, String(value));
  };

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

  // Effective tab: the remembered pick when it still exists, else the newest
  // artifact (last in the list), else Notes.
  const remembered = selectedByWorkspace.get(workspace.id);
  const selected: TabId =
    remembered !== undefined &&
    (remembered === "notes" || artifacts.some((a) => a.id === remembered))
      ? remembered
      : (artifacts[artifacts.length - 1]?.id ?? "notes");
  const selectedArtifact = artifacts.find((a) => a.id === selected) ?? null;

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
        width={width}
        onResize={(w) => {
          setWidth(w);
          localStorage.setItem(WIDTH_KEY, String(w));
        }}
      />
      <div className="side-tabs" role="tablist">
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
        {selectedArtifact === null ? (
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
function ResizeHandle({
  width,
  onResize,
}: {
  width: number;
  onResize: (width: number) => void;
}) {
  const onMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = width;
    const max = Math.floor(window.innerWidth * 0.7);
    const onMove = (ev: MouseEvent) => {
      const next = startWidth + (startX - ev.clientX);
      onResize(Math.max(MIN_WIDTH, Math.min(max, next)));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "col-resize";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };
  return <div className="side-resize" onMouseDown={onMouseDown} />;
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
