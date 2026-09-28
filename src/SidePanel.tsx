import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import * as api from "./ipc/commands";
import { useAppEvent } from "./ipc/events";
import { useTheme } from "./theme";
import type { Artifact, Workspace } from "./types";
import { useHorizontalScroll } from "./useHorizontalScroll";

const WIDTHS_KEY = "tethys.sidePanel.widths";
const COLLAPSED_KEY = "tethys.sidePanel.collapsedByWorkspace";
// A percentage, so it stays an even split as the window resizes.
const DEFAULT_WIDTH = "50%";
const MIN_WIDTH = 280;

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
    /* unparseable starts fresh */
  }
  return {};
}

const isPanelWidth = (v: unknown): v is number =>
  typeof v === "number" && v >= MIN_WIDTH;
const isBoolean = (v: unknown): v is boolean => typeof v === "boolean";
// The native PR webview trails the DOM by a frame or two during a fast drag;
// pulling its edge back this far keeps the cursor off it.
const DRAG_GUARD_PX = 64;

// "notes", an artifact id, or a `pr:<url>`.
type TabId = "notes" | string;

interface PrTab {
  id: string;
  number: number;
  url: string;
  repoKey: string;
}

interface Props {
  workspace: Workspace;
  notes: string;
  onNotesChange: (notes: string) => void;
}

export function SidePanel({ workspace, notes, onNotesChange }: Props) {
  const [resizing, setResizing] = useState(false);
  const [collapsedMap, setCollapsedMap] = useState(() =>
    loadMap(COLLAPSED_KEY, isBoolean),
  );
  const collapsed = collapsedMap[workspace.id] ?? true;
  const persistCollapsed = useCallback(
    (value: boolean) => {
      setCollapsedMap((prev) => {
        const next = { ...prev, [workspace.id]: value };
        localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next));
        return next;
      });
    },
    [workspace.id],
  );
  const [widths, setWidths] = useState(() =>
    loadMap(WIDTHS_KEY, isPanelWidth),
  );
  const width: number | string = widths[workspace.id] ?? DEFAULT_WIDTH;
  const setWorkspaceWidth = (w: number) => {
    setWidths((prev) => {
      const next = { ...prev, [workspace.id]: w };
      localStorage.setItem(WIDTHS_KEY, JSON.stringify(next));
      return next;
    });
  };
  // Tagged so a list still in flight for the previous workspace reads as
  // "not loaded" rather than empty.
  const [artifactState, setArtifactState] = useState<{
    workspaceId: string;
    list: Artifact[];
  } | null>(null);
  const artifactsLoaded = artifactState?.workspaceId === workspace.id;
  const artifacts = useMemo(
    () => (artifactsLoaded ? artifactState.list : []),
    [artifactsLoaded, artifactState],
  );
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
    const workspaceId = workspace.id;
    api
      .listArtifacts(workspaceId)
      .then((list) => setArtifactState({ workspaceId, list }))
      .catch((e) => console.error("list_artifacts failed:", e));
  }, [workspace.id]);

  useEffect(() => {
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

  // A never-fetched PR has no URL to load, so it gets no tab.
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

  const newestTab: TabId =
    artifacts[artifacts.length - 1]?.id ?? prTabs[0]?.id ?? "notes";

  const remembered = selectedByWorkspace.get(workspace.id);
  const selected: TabId =
    remembered !== undefined &&
    (remembered === "notes" ||
      artifacts.some((a) => a.id === remembered) ||
      prTabs.some((t) => t.id === remembered))
      ? remembered
      : notes.trim()
        ? "notes"
        : newestTab;

  // A panel that had only Notes and just gained a tab opens onto it: a
  // collapse chosen while it was empty wasn't a choice about this tab.
  // Compared only once artifacts load, so booting or switching workspaces
  // never reads as a tab appearing.
  const tabCount = artifactsLoaded ? artifacts.length + prTabs.length : null;
  const seenTabCounts = useRef(new Map<string, number>());
  useEffect(() => {
    if (tabCount === null) return;
    const prev = seenTabCounts.current.get(workspace.id);
    seenTabCounts.current.set(workspace.id, tabCount);
    if (prev === 0 && tabCount > 0) {
      select(newestTab);
      persistCollapsed(false);
    }
  }, [workspace.id, tabCount, newestTab, select, persistCollapsed]);
  const selectedArtifact = artifacts.find((a) => a.id === selected) ?? null;
  const selectedPr = prTabs.find((t) => t.id === selected) ?? null;

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
    if (selected === id) {
      const i = artifacts.findIndex((a) => a.id === id);
      const next = artifacts[i + 1] ?? artifacts[i - 1];
      select(next ? next.id : "notes");
    }
    setArtifactState((prev) =>
      prev && { ...prev, list: prev.list.filter((a) => a.id !== id) },
    );
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

// Pointer capture can't cross the native PR webview, which swallows events at
// the OS level, so a release over it is never seen: a move with no button held
// ends the drag, and so does a window blur.
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
    // Measured: an undragged panel's width is a percentage.
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
  const pending = useRef<string | null>(null);

  const save = useCallback(
    (notes: string) => {
      pending.current = null;
      api.setWorkspaceNotes(workspaceId, notes).catch(() => {});
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
  // A couple of megabytes; loaded on first use.
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

// GitHub refuses to be framed, so the page is a native child webview
// (`pr_view.rs`) floated over the host `<div>`; this owns only its geometry.
function PrView({
  url,
  number,
  repoKey,
  resizing,
}: {
  url: string;
  number: number;
  repoKey: string;
  resizing: boolean;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const modalOpen = useModalOpen();

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    // A native view sits above every DOM layer, dialogs included.
    if (modalOpen) {
      api.hidePrView().catch(() => {});
      return;
    }
    const inset = resizing ? DRAG_GUARD_PX : 0;
    const sync = () => {
      const r = host.getBoundingClientRect();
      if (r.width - inset < 2 || r.height < 2) {
        api.hidePrView().catch(() => {});
        return;
      }
      api
        .showPrView(
          url,
          {
            x: r.left + inset,
            y: r.top,
            width: r.width - inset,
            height: r.height,
          },
          window.innerHeight,
        )
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
  }, [url, resizing, modalOpen]);

  // Separate from the sync effect so switching PR tabs never flashes hidden.
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

function useModalOpen(): boolean {
  const query = () => document.querySelector(".modal-backdrop") !== null;
  const [open, setOpen] = useState(query);
  useEffect(() => {
    const observer = new MutationObserver(() => setOpen(query()));
    observer.observe(document.body, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, []);
  return open;
}

function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

function prefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function isDark(hex: string): boolean {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(hex);
  if (!m) return prefersDark();
  const [r, g, b] = [m[1], m[2], m[3]].map((h) => parseInt(h, 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5;
}
