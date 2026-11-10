import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import * as api from "./ipc/commands";
import { useAppEvent } from "./ipc/events";
import { GithubChip, PrDetachButton } from "./GithubChip";
import { useTheme } from "./theme";
import type { Artifact, GithubPrStatus, Workspace } from "./types";
import { useHorizontalScroll } from "./useHorizontalScroll";
import { linkPrEntries, prGroups } from "./workspaceDerived";

// "chat", an artifact id, or a `pr:<repo>#<number>`.
export type TabId = "chat" | string;

interface PrTab {
  id: string;
  number: number;
  repoKey: string;
  /** `null` until the PR has been fetched. */
  status: GithubPrStatus | null;
}

/** A `gh stack`'s tracked members share one outline, base-first. */
interface PrTabGroup {
  stack: { number: number; size: number } | null;
  tabs: PrTab[];
}

export interface WorkspaceTabs {
  selected: TabId;
  select: (tab: TabId) => void;
  artifacts: Artifact[];
  prTabs: PrTab[];
  prTabGroups: PrTabGroup[];
  dismiss: (id: string) => void;
}

export function useWorkspaceTabs(workspace: Workspace): WorkspaceTabs {
  // Tagged so a list still in flight for the previous workspace reads as
  // "not loaded" rather than empty.
  const [artifactState, setArtifactState] = useState<{
    workspaceId: string;
    list: Artifact[];
  } | null>(null);
  const artifacts = useMemo(
    () =>
      artifactState?.workspaceId === workspace.id ? artifactState.list : [],
    [workspace.id, artifactState],
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
    if (payload.artifact_id) select(payload.artifact_id);
  });

  const prTabGroups = useMemo<PrTabGroup[]>(() => {
    const groups: PrTabGroup[] = [];
    for (const link of workspace.repo_links) {
      const tab = (number: number, status: GithubPrStatus | null): PrTab => ({
        id: `pr:${link.repo_key}#${number}`,
        number,
        repoKey: link.repo_key,
        status,
      });
      for (const g of prGroups(linkPrEntries(link))) {
        groups.push({
          stack: g.stack,
          tabs: g.prs.map((e) => tab(e.number, e.status)),
        });
      }
      for (const pr of link.prs) {
        if (!pr.status)
          groups.push({ stack: null, tabs: [tab(pr.number, null)] });
      }
    }
    return groups;
  }, [workspace.repo_links]);
  const prTabs = useMemo(
    () => prTabGroups.flatMap((g) => g.tabs),
    [prTabGroups],
  );

  const remembered = selectedByWorkspace.get(workspace.id);
  const selected: TabId =
    remembered !== undefined &&
    (artifacts.some((a) => a.id === remembered) ||
      prTabs.some((t) => t.id === remembered))
      ? remembered
      : "chat";

  const dismiss = (id: string) => {
    if (selected === id) {
      const i = artifacts.findIndex((a) => a.id === id);
      const next = artifacts[i + 1] ?? artifacts[i - 1];
      select(next ? next.id : "chat");
    }
    setArtifactState(
      (prev) => prev && { ...prev, list: prev.list.filter((a) => a.id !== id) },
    );
    api
      .dismissArtifact(workspace.id, id)
      .catch((e) => console.error("dismiss_artifact failed:", e));
  };

  return { selected, select, artifacts, prTabs, prTabGroups, dismiss };
}

export function TabStrip({
  tabs,
  chatAttention,
  onDetachPr,
}: {
  tabs: WorkspaceTabs;
  chatAttention: boolean;
  onDetachPr: (repoKey: string, prNumber: number) => void;
}) {
  const { selected, select, artifacts, prTabGroups, dismiss } = tabs;

  const prTab = (t: PrTab) => (
    <div
      key={t.id}
      role="tab"
      className={`main-tab pr ${selected === t.id ? "active" : ""}`}
      onClick={() => select(t.id)}
    >
      {t.status ? (
        <GithubChip
          status={t.status}
          linkable={false}
          context={t.repoKey}
          onDetach={() => onDetachPr(t.repoKey, t.number)}
        />
      ) : (
        <span
          className="gh-chip gh-chip-static gh-chip-missing"
          title={`PR #${t.number} in ${t.repoKey} couldn't be fetched`}
        >
          <span className="gh-pr">{t.number}</span>
          <span className="gh-note-badge">no data</span>
          <PrDetachButton
            prNumber={t.number}
            onDetach={() => onDetachPr(t.repoKey, t.number)}
          />
        </span>
      )}
    </div>
  );
  const strip = useRef<HTMLDivElement | null>(null);
  const wheelRef = useHorizontalScroll<HTMLDivElement>();
  const ref = useCallback(
    (el: HTMLDivElement | null) => {
      strip.current = el;
      wheelRef(el);
    },
    [wheelRef],
  );
  useEffect(() => {
    strip.current
      ?.querySelector(".main-tab.active")
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [selected]);

  return (
    <div className="main-tabs" role="tablist" ref={ref}>
      <button
        type="button"
        role="tab"
        className={`main-tab ${selected === "chat" ? "active" : ""}`}
        onClick={() => select("chat")}
      >
        Chat
        {chatAttention && selected !== "chat" && (
          <span className="side-rail-dot" />
        )}
      </button>
      {prTabGroups.map((g) =>
        g.stack ? (
          <span
            key={`stack-${g.tabs[0].repoKey}-${g.stack.number}`}
            className="main-tab-stack"
            title={stackTitle(g)}
          >
            {g.tabs.map((t, i) => (
              <Fragment key={t.id}>
                {i > 0 && (
                  <span className="gh-stack-sep" aria-hidden="true">
                    ›
                  </span>
                )}
                {prTab(t)}
              </Fragment>
            ))}
            {g.tabs.length < g.stack.size && (
              <span className="gh-stack-count">
                {g.tabs.map((t) => t.status?.stack?.position).join(", ")} of{" "}
                {g.stack.size}
              </span>
            )}
          </span>
        ) : (
          prTab(g.tabs[0])
        ),
      )}
      {artifacts.map((a) => (
        <div
          key={a.id}
          role="tab"
          className={`main-tab artifact ${selected === a.id ? "active" : ""}`}
          onClick={() => select(a.id)}
          title={
            a.kind === "page" ? a.path : a.kind === "review" ? a.url : a.label
          }
        >
          <span className="main-tab-glyph">
            {ARTIFACT_GLYPHS[a.kind]}
          </span>
          <span className="main-tab-label">{a.label}</span>
          <button
            type="button"
            className="main-tab-close"
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
    </div>
  );
}

function stackTitle(g: PrTabGroup): string {
  const members = g.tabs
    .map((t) => `#${t.number} (${t.status?.stack?.position})`)
    .join(" → ");
  return `Stack #${g.stack!.number}, ${g.stack!.size} PRs, base-first: ${members}`;
}

/** Whatever the selected non-chat tab shows. */
export function TabContent({
  workspaceId,
  tabs,
}: {
  workspaceId: string;
  tabs: WorkspaceTabs;
}) {
  const pr = tabs.prTabs.find((t) => t.id === tabs.selected);
  if (pr) {
    if (!pr.status) {
      return (
        <div className="artifact-pending">
          PR #{pr.number} in {pr.repoKey} couldn't be fetched yet.
        </div>
      );
    }
    return (
      <PrView
        url={pr.status.url}
        number={pr.number}
        repoKey={pr.repoKey}
      />
    );
  }
  const artifact = tabs.artifacts.find((a) => a.id === tabs.selected);
  if (!artifact) return null;
  switch (artifact.kind) {
    case "diagram":
      return <DiagramView key={artifact.id} source={artifact.source} />;
    case "page":
      return <PageView workspaceId={workspaceId} artifact={artifact} />;
    case "review":
      return <ReviewView artifact={artifact} />;
  }
}

const ARTIFACT_GLYPHS: Record<Artifact["kind"], string> = {
  diagram: "◇",
  page: "▤",
  review: "≣",
};

// ummon's own page, served from localhost. Not sandboxed: it posts to its own
// server, which checks the request's origin.
function ReviewView({ artifact }: { artifact: Artifact & { kind: "review" } }) {
  return (
    <div className="artifact-view">
      <div className="artifact-toolbar">
        <span className="artifact-toolbar-title" title={artifact.url}>
          {artifact.label}
        </span>
        <button
          type="button"
          onClick={() =>
            openUrl(artifact.url).catch((e) =>
              console.error("openUrl failed:", e),
            )
          }
        >
          Open in browser
        </button>
      </div>
      <iframe
        key={artifact.revision}
        className="artifact-page"
        title={artifact.label}
        src={artifact.url}
      />
    </div>
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

// GitHub refuses to be framed, so this positions a native webview
// (`pr_view.rs`) over the host `<div>`.
function PrView({
  url,
  number,
  repoKey,
}: {
  url: string;
  number: number;
  repoKey: string;
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
    const sync = () => {
      const r = host.getBoundingClientRect();
      if (r.width < 2 || r.height < 2) {
        api.hidePrView().catch(() => {});
        return;
      }
      api
        .showPrView(
          url,
          {
            x: r.left,
            y: r.top,
            width: r.width,
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
  }, [url, modalOpen]);

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
