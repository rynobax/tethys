import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import * as api from "./ipc/commands";
import type {
  Agent,
  CreateWorkspaceArgs,
  Discrepancies,
  Folder,
  FolderId,
  JobEvent,
  RegistryStatus,
  Repo,
  RepoLink,
  SessionInfo,
  Theme,
  Workspace,
  WorkspaceId,
} from "./types";
import { GithubAuthFooter } from "./GithubAuthFooter";
import { GithubChip, PrDetachButton } from "./GithubChip";
import { JobLogPane } from "./JobLogPane";
import { SessionTerminal } from "./SessionTerminal";
import { SidePanel } from "./SidePanel";
import { Sidebar } from "./Sidebar";
import { SystemStatus } from "./SystemStatus";
import { applyTheme, ThemeContext } from "./theme";
import {
  useBackendJob,
  type JobDescriptor,
  type JobState,
} from "./useBackendJob";
import { useHorizontalScroll } from "./useHorizontalScroll";
import { useAppEvent } from "./ipc/events";
import {
  linkPrEntries,
  prGroups,
  type LinkPr,
  type PrGroup,
} from "./workspaceDerived";
import "./App.css";

/** The agent CLIs a workspace can run, shared by the new-workspace form and
 *  the workspace header's "run with" switcher. First entry is the default.
 *
 *  One list, two fields: the binary is what gets executed, the agent is what
 *  decides how it's spawned, resumed and hooked. The agent is carried here
 *  rather than derived from the name, so a differently-spelled wrapper (or a
 *  second `claude-*` variant) can't be mistaken for the wrong harness. */
const AGENT_CHOICES = [
  { binary: "claude", agent: "claude" },
  { binary: "claude-hipaa", agent: "claude" },
  { binary: "claude-unsafe", agent: "claude" },
  { binary: "codex", agent: "codex" },
] as const satisfies readonly { binary: string; agent: Agent }[];

/** The agent a binary name belongs to. Falls back to Claude for a name the
 *  list no longer carries — a workspace pinned to a retired binary keeps
 *  working rather than becoming unopenable. */
const agentFor = (binary: string): Agent =>
  AGENT_CHOICES.find((c) => c.binary === binary)?.agent ?? "claude";

/** Bracketed-paste markers: Claude Code treats the wrapped bytes as pasted
 *  text rather than typed keystrokes, so the draft lands in the prompt box
 *  without being submitted. Mirrors the drag-drop paste in SessionTerminal. */
const PASTE_START = "\x1b[200~";
const PASTE_END = "\x1b[201~";
/** Give Claude's TUI a beat to mount its input box after the SessionStart
 *  hook fires before pasting, so the draft isn't swallowed by the startup
 *  redraw. */
const DRAFT_PROMPT_SETTLE_MS = 500;

/**
 * One in-flight (or just-settled) add-repo job, as the detail pane needs to
 * draw it. Lives in `App` so the work outlives the pane that started it.
 */
interface AddRepoRun {
  /** Identifies this invocation, so a dismissed job's late events are
   *  dropped instead of landing on whatever replaced it. */
  key: string;
  repoKey: string;
  events: JobEvent[];
  state: JobState;
}

function App() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [registry, setRegistry] = useState<RegistryStatus | null>(null);
  const [discrepancies, setDiscrepancies] = useState<Discrepancies | null>(
    null,
  );
  const [selectedId, setSelectedId] = useState<WorkspaceId | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /**
   * Args for create_workspace invocations the runner is currently driving,
   * keyed by workspace_id. The backend inserts a `Creating` draft into
   * `workspaces` from t=0, so this map only carries the args the runner
   * needs to pass to invoke; sidebar position lives entirely in `workspaces`.
   * Entries are removed on success (after auto-start) or on user dismissal.
   */
  const [creationRuns, setCreationRuns] = useState<
    Map<WorkspaceId, CreateWorkspaceArgs>
  >(new Map());
  /**
   * In-flight `add_repo_to_workspace` invocations, keyed by workspace id.
   *
   * The job is driven from here rather than from inside the workspace's
   * detail pane, because a pane unmounts the moment you select another
   * workspace — which is why adding a repo used to be a modal you had to sit
   * and watch. The channel writes straight into this map, so nothing has to
   * stay mounted for the log to keep accumulating; the detail pane only
   * *displays* the run it is handed, as a popup over its own header.
   *
   * `key` is minted per invocation and captured by the channel closure, so
   * events from a job the user has already dismissed can't land on the run
   * that replaced it.
   */
  const [addRepoRuns, setAddRepoRuns] = useState<Map<WorkspaceId, AddRepoRun>>(
    new Map(),
  );
  /**
   * Per-workspace attention state, tracked by listening to
   * `session:turn_changed` globally so the sidebar dot doesn't need a
   * session fetch per workspace.
   *
   * Stores the backend's derived answers rather than the raw
   * running/runtime_state/turn_acknowledged triple. Deriving it here is what
   * let the sidebar and the detail pane drift apart.
   */
  const [turnStates, setTurnStates] = useState<
    Map<WorkspaceId, { needsTurn: boolean; working: boolean }>
  >(new Map());
  /**
   * Each workspace's live session, cached so switching into a workspace
   * shows the terminal immediately instead of flashing "Dormant" during
   * the get_session round-trip. Populated eagerly on workspace load and
   * kept in sync via session:* events. `null` is a real answer: dormant.
   */
  const [sessionByWorkspace, setSessionByWorkspace] = useState<
    Map<WorkspaceId, SessionInfo | null>
  >(new Map());
  /**
   * Draft "initial prompt" text the user types while a workspace is still
   * provisioning, keyed by workspace_id. Once that workspace's first agent
   * session reports a `agent_session_id` (its SessionStart hook fired, so the
   * TUI is up), the draft is pasted into the session — bracketed paste, no
   * submit — and the entry is dropped.
   */
  const [draftPrompts, setDraftPrompts] = useState<Map<WorkspaceId, string>>(
    new Map(),
  );
  /**
   * Live notes text per workspace, keyed by workspace_id. The notes editor
   * remounts on every workspace switch, and `workspaces[].notes` can't seed it
   * on the way back in: `set_workspace_notes` deliberately doesn't emit
   * `workspace:changed` (that would churn the pane on every keystroke), so the
   * copy in `workspaces` stays at whatever the last `refresh()` read — stale
   * the moment the user types. This map is the authoritative text while the app
   * is running; the backend still gets debounced writes for restarts.
   */
  const [noteDrafts, setNoteDrafts] = useState<Map<WorkspaceId, string>>(
    new Map(),
  );
  /**
   * Workspaces whose draft prompt has already been pasted (or is mid-paste),
   * so the flush effect doesn't double-send on repeated `workspace:changed`.
   */
  const flushedDraftsRef = useRef<Set<WorkspaceId>>(new Set());
  const [theme, setTheme] = useState<Theme | null>(null);

  useEffect(() => {
    api
      .getTheme()
      .then((t) => {
        setTheme(t);
        applyTheme(t);
      })
      .catch((e) => console.error("get_theme failed:", e));
  }, []);

  useAppEvent("theme:changed", (payload) => {
    const t = payload ?? null;
    setTheme(t);
    applyTheme(t);
  });

  useAppEvent("session:turn_changed", (payload) => {
    const {
      workspace_id,
      session_id,
      runtime_state,
      notification_type,
      turn_acknowledged,
      running,
      needs_turn,
      working,
    } = payload;
    setTurnStates((prev) => {
      const next = new Map(prev);
      next.set(workspace_id, { needsTurn: needs_turn, working });
      return next;
    });
    // Keep the cached SessionInfo in sync so WorkspaceDetail sees the new
    // runtime_state without a full re-fetch. A signal for a session that is
    // no longer the workspace's (replaced by a binary switch) is skipped.
    setSessionByWorkspace((prev) => {
      const s = prev.get(workspace_id);
      if (!s || s.id !== session_id) return prev;
      const next = new Map(prev);
      next.set(workspace_id, {
        ...s,
        runtime_state,
        notification_type: notification_type ?? null,
        turn_acknowledged,
        running,
        needs_turn,
        working,
      });
      return next;
    });
  });

  useAppEvent("github:status_changed", (payload) => {
    const { workspace_id, repo_key, pr_number, status } = payload;
    setWorkspaces((prev) =>
      prev.map((w) => {
        if (w.id !== workspace_id) return w;
        return {
          ...w,
          repo_links: w.repo_links.map((r) => {
            if (r.repo_key !== repo_key) return r;
            // Every status names its PR, and every PR lives in one list, so
            // there's no slot to pick between. A number we don't track was
            // detached mid-tick; the backend already dropped it.
            return {
              ...r,
              prs: r.prs.map((p) =>
                p.number === pr_number ? { ...p, status } : p,
              ),
            };
          }),
        };
      }),
    );
  });

  const workspaceNeedsTurn = useCallback(
    (w: Workspace): boolean => turnStates.get(w.id)?.needsTurn ?? false,
    [turnStates],
  );

  const workspaceWorking = useCallback(
    (w: Workspace): boolean => turnStates.get(w.id)?.working ?? false,
    [turnStates],
  );

  const handleClearTurn = useCallback((workspace: Workspace) => {
    // Backend persists turn_acknowledged + emits session:turn_changed
    // back, which updates turnStates. No optimistic local update needed —
    // the round-trip is fast and the persisted flag is the source of truth.
    api
      .acknowledgeSessionTurn(workspace.id)
      .catch((e) => console.error("acknowledge_session_turn failed:", e));
  }, []);

  const refreshSessionFor = useCallback(async (workspaceId: WorkspaceId) => {
    try {
      const session = await api.getSession(workspaceId);
      setSessionByWorkspace((prev) => {
        const next = new Map(prev);
        next.set(workspaceId, session);
        return next;
      });
      // Seed turnStates from the snapshot. The backend restores turn state
      // from disk at boot but deliberately emits nothing (the frontend isn't
      // subscribed yet) — without this the sidebar dot stays dark across
      // restarts until the next live event fires.
      //
      // This used to race the `dormant` event handler, which deleted the
      // entry while this re-inserted a stale one. Both sides now read the
      // same derived flags, so they can't disagree.
      setTurnStates((prev) => {
        const needsTurn = session?.needs_turn ?? false;
        const working = session?.working ?? false;
        const cur = prev.get(workspaceId);
        if (cur && cur.needsTurn === needsTurn && cur.working === working) {
          return prev;
        }
        const next = new Map(prev);
        next.set(workspaceId, { needsTurn, working });
        return next;
      });
    } catch (e) {
      console.error("get_session:", e);
    }
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [list, folderList, reg, disc] = await Promise.all([
        api.listWorkspaces(),
        api.listFolders(),
        api.registryStatus(),
        api.listDiscrepancies(),
      ]);
      setWorkspaces(list);
      setFolders(folderList);
      setRegistry(reg);
      setDiscrepancies(disc);
      setError(null);
      // Pre-load every workspace's session so switching in doesn't render
      // a stale "Dormant" pane.
      await Promise.all(list.map((w) => refreshSessionFor(w.id)));
    } catch (e) {
      setError(String(e));
    }
  }, [refreshSessionFor]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useAppEvent("workspace:changed", () => refresh());
  useAppEvent("session:changed", (payload) => {
    refreshSessionFor(payload.workspace_id);
  });
  useAppEvent("session:exit", (payload) => {
    refreshSessionFor(payload.workspace_id);
  });

  // Paste any draft initial-prompt into a workspace's agent session once
  // it's up. `workspace:changed` fires (and refreshes `workspaces`) when the
  // SessionStart hook populates `agent_session_id`, which is our signal
  // that the TUI is ready to receive a paste.
  useEffect(() => {
    for (const [workspaceId, prompt] of draftPrompts) {
      if (flushedDraftsRef.current.has(workspaceId)) continue;
      if (prompt.trim().length === 0) continue;
      const ws = workspaces.find((w) => w.id === workspaceId);
      if (!ws || ws.status.kind !== "ready") continue;
      const session = ws.session;
      if (!session || session.agent_session_id === null) continue;

      flushedDraftsRef.current.add(workspaceId);
      const sessionId = session.id;
      const bytes = Array.from(
        new TextEncoder().encode(`${PASTE_START}${prompt}${PASTE_END}`),
      );
      const flush = async () => {
        await new Promise((resolve) =>
          setTimeout(resolve, DRAFT_PROMPT_SETTLE_MS),
        );
        try {
          await api.sendInput(sessionId, bytes);
        } catch (e) {
          console.error("flush draft prompt failed:", e);
          // Let a later `workspace:changed` retry the paste.
          flushedDraftsRef.current.delete(workspaceId);
          return;
        }
        setDraftPrompts((prev) => {
          if (!prev.has(workspaceId)) return prev;
          const next = new Map(prev);
          next.delete(workspaceId);
          return next;
        });
      };
      void flush();
    }
  }, [workspaces, draftPrompts]);

  const visibleWorkspaces = useMemo(
    () => workspaces.filter((w) => !w.deleted_at),
    [workspaces],
  );
  const selected = useMemo(() => {
    const ws = workspaces.find((w) => w.id === selectedId);
    if (!ws) return null;
    if (ws.deleted_at) return null;
    return ws;
  }, [workspaces, selectedId]);

  const handleCreateSuccess = useCallback(
    async (workspaceId: WorkspaceId, result: unknown) => {
      const ws = result as Workspace;
      // Tear down the runner now that provisioning is done — the workspace
      // already lives in `workspaces` with status=Ready, so the detail
      // pane swaps from JobLogPane to WorkspaceDetail naturally.
      setCreationRuns((prev) => {
        if (!prev.has(workspaceId)) return prev;
        const next = new Map(prev);
        next.delete(workspaceId);
        return next;
      });
      // Auto-start the workspace's agent session. Where it runs — the
      // only repo's worktree, or the workspace root — is the backend's
      // call (`Workspace::session_cwd`).
      try {
        await api.startAgentSession(ws.id);
      } catch (e) {
        setError(`auto-start failed: ${String(e)}`);
      }
    },
    [],
  );

  const handleCreationDismiss = useCallback(
    async (workspaceId: WorkspaceId) => {
      setCreationRuns((prev) => {
        if (!prev.has(workspaceId)) return prev;
        const next = new Map(prev);
        next.delete(workspaceId);
        return next;
      });
      // Drop any draft prompt the user typed for this (now-abandoned) workspace.
      setDraftPrompts((prev) => {
        if (!prev.has(workspaceId)) return prev;
        const next = new Map(prev);
        next.delete(workspaceId);
        return next;
      });
      flushedDraftsRef.current.delete(workspaceId);
      setSelectedId((cur) => (cur === workspaceId ? null : cur));
      // Drop the failed draft from state. `forget_workspace` is a hard
      // delete with no grace window — the right call here since there are
      // no worktrees on disk for a CreationFailed entry (the backend
      // already tore them down) and no purger semantics to preserve.
      try {
        await api.forgetWorkspace(workspaceId);
      } catch (e) {
        // Workspace may already be gone (e.g. invoke rejected before the
        // draft was even inserted) — not fatal, just log and move on.
        console.warn("forget_workspace failed:", e);
      }
    },
    [],
  );

  /**
   * Kick off `add_repo_to_workspace` for a workspace and stream its events
   * into `addRepoRuns`. Nothing about this is tied to the detail pane, so
   * the user is free to work in another workspace while it provisions —
   * and to come back to a finished (or failed) log when they return.
   */
  const startAddRepo = useCallback(
    (workspaceId: WorkspaceId, repoKey: string) => {
      const runKey = crypto.randomUUID();
      const patch = (update: (run: AddRepoRun) => AddRepoRun) =>
        setAddRepoRuns((prev) => {
          const cur = prev.get(workspaceId);
          // A run the user dismissed, or one replaced by a later attempt.
          if (!cur || cur.key !== runKey) return prev;
          const next = new Map(prev);
          next.set(workspaceId, update(cur));
          return next;
        });

      setAddRepoRuns((prev) => {
        const next = new Map(prev);
        next.set(workspaceId, {
          key: runKey,
          repoKey,
          events: [],
          state: "running",
        });
        return next;
      });

      const channel = new api.Channel<JobEvent>();
      channel.onmessage = (event) => {
        patch((run) => ({
          ...run,
          events: [...run.events, event],
          state:
            event.kind === "success"
              ? "success"
              : event.kind === "failed"
                ? "failed"
                : run.state,
        }));
      };

      const { command, args } = api.jobs.addRepoToWorkspace({
        workspace_id: workspaceId,
        repo_key: repoKey,
      });
      api
        .runJob(command, args, channel)
        .then(() => {
          patch((run) => ({
            ...run,
            state: run.state === "running" ? "success" : run.state,
          }));
          void refresh();
        })
        .catch((e) => {
          patch((run) => ({
            ...run,
            events:
              run.events[run.events.length - 1]?.kind === "failed"
                ? run.events
                : [...run.events, { kind: "failed", error: String(e) }],
            state: run.state === "running" ? "failed" : run.state,
          }));
        });
    },
    [refresh],
  );

  const dismissAddRepo = useCallback((workspaceId: WorkspaceId) => {
    setAddRepoRuns((prev) => {
      if (!prev.has(workspaceId)) return prev;
      const next = new Map(prev);
      next.delete(workspaceId);
      return next;
    });
  }, []);

  const handleDelete = useCallback(async (workspace: Workspace) => {
    setSelectedId((cur) => (cur === workspace.id ? null : cur));
    // CreationFailed entries have no worktrees on disk, so skip the
    // soft-delete + 1-hour grace window and just drop from state.
    try {
      await (workspace.status.kind === "creation_failed"
        ? api.forgetWorkspace(workspace.id)
        : api.deleteWorkspace(workspace.id));
    } catch (e) {
      setError(`delete failed: ${String(e)}`);
    }
  }, []);

  // Folders are only ever written from here, so the local list is kept in
  // step by hand rather than by a round-trip: same reason `handleReorder`
  // does it, and it means a rename or a collapse repaints instantly.
  const handleCreateFolder = useCallback(async (name: string) => {
    try {
      const folder = await api.createFolder(name);
      setFolders((prev) => [...prev, folder]);
    } catch (e) {
      setError(`could not create folder: ${String(e)}`);
    }
  }, []);

  const handleRenameFolder = useCallback(async (id: FolderId, name: string) => {
    setFolders((prev) => prev.map((f) => (f.id === id ? { ...f, name } : f)));
    try {
      await api.renameFolder(id, name);
    } catch (e) {
      setError(`could not rename folder: ${String(e)}`);
    }
  }, []);

  const handleDeleteFolder = useCallback(async (id: FolderId) => {
    // Its workspaces fall back to Default — mirrored locally so the rows
    // reappear at the top rather than vanishing until the next refresh.
    setFolders((prev) => prev.filter((f) => f.id !== id));
    setWorkspaces((prev) =>
      prev.map((w) => (w.folder === id ? { ...w, folder: null } : w)),
    );
    try {
      await api.deleteFolder(id);
    } catch (e) {
      setError(`could not delete folder: ${String(e)}`);
    }
  }, []);

  const handleSetFolderCollapsed = useCallback(
    async (id: FolderId, collapsed: boolean) => {
      setFolders((prev) =>
        prev.map((f) => (f.id === id ? { ...f, collapsed } : f)),
      );
      try {
        await api.setFolderCollapsed(id, collapsed);
      } catch (e) {
        setError(`could not collapse folder: ${String(e)}`);
      }
    },
    [],
  );

  const handleReorderFolders = useCallback(async (ids: FolderId[]) => {
    setFolders((prev) => {
      const byId = new Map(prev.map((f) => [f.id, f]));
      const moved = ids.flatMap((id) => byId.get(id) ?? []);
      const seen = new Set(ids);
      return [...moved, ...prev.filter((f) => !seen.has(f.id))];
    });
    try {
      await api.reorderFolders(ids);
    } catch (e) {
      setError(`could not reorder folders: ${String(e)}`);
    }
  }, []);

  const handleSetBlocker = useCallback(
    async (workspace: Workspace, blockerId: WorkspaceId | null) => {
      try {
        await api.setWorkspaceBlocker(workspace.id, blockerId);
      } catch (e) {
        setError(`could not set blocker: ${String(e)}`);
      }
    },
    [],
  );

  const handleReorder = useCallback(async (ids: WorkspaceId[]) => {
    // Optimistically reorder so the drop animation lands on the right row,
    // then fire the backend command. This local update is the only thing that
    // repaints the sidebar — the backend emits nothing for a reorder, because
    // a round-trip would flicker the row that was just dropped.
    setWorkspaces((prev) => {
      const byId = new Map(prev.map((w) => [w.id, w]));
      const moved: Workspace[] = [];
      for (const id of ids) {
        const w = byId.get(id);
        if (w) moved.push(w);
      }
      const idsSet = new Set(ids);
      const rest = prev.filter((w) => !idsSet.has(w.id));
      return [...moved, ...rest];
    });
    try {
      await api.reorderWorkspaces(ids);
    } catch (e) {
      setError(`reorder failed: ${String(e)}`);
    }
  }, []);

  /** A drag that crossed a folder boundary: the same optimistic-then-tell-the-
   *  backend shape as a plain reorder, with the membership write alongside. */
  const handleMoveToFolder = useCallback(
    async (
      ids: WorkspaceId[],
      folder: FolderId | null,
      order: WorkspaceId[],
    ) => {
      const movedSet = new Set(ids);
      setWorkspaces((prev) => {
        const byId = new Map(
          prev.map((w) => [w.id, movedSet.has(w.id) ? { ...w, folder } : w]),
        );
        const moved = order.flatMap((id) => byId.get(id) ?? []);
        const seen = new Set(order);
        return [...moved, ...prev.filter((w) => !seen.has(w.id))];
      });
      try {
        await api.moveWorkspacesToFolder(ids, folder);
        await api.reorderWorkspaces(order);
      } catch (e) {
        setError(`could not move workspace: ${String(e)}`);
      }
    },
    [],
  );

  const registryOk = registry?.kind === "ok";
  const selectedRun = selectedId
    ? (creationRuns.get(selectedId) ?? null)
    : null;

  return (
    <ThemeContext.Provider value={theme}>
      <div className="app">
        <aside className="sidebar">
          <div className="sidebar-header">
            <button
              className="new-workspace"
              onClick={() => setCreating(true)}
              type="button"
              disabled={!registryOk}
              title={!registryOk ? "Configure repos.toml first" : undefined}
            >
              <span className="new-workspace-plus" aria-hidden="true">
                +
              </span>
              New workspace
            </button>
          </div>
          <Sidebar
            workspaces={visibleWorkspaces}
            folders={folders}
            selectedId={selectedId}
            onSelect={setSelectedId}
            onReorder={handleReorder}
            onMoveToFolder={handleMoveToFolder}
            onReorderFolders={handleReorderFolders}
            onCreateFolder={handleCreateFolder}
            onRenameFolder={handleRenameFolder}
            onDeleteFolder={handleDeleteFolder}
            onSetFolderCollapsed={handleSetFolderCollapsed}
            onDelete={handleDelete}
            onClearTurn={handleClearTurn}
            onSetBlocker={handleSetBlocker}
            workspaceNeedsTurn={workspaceNeedsTurn}
            workspaceWorking={workspaceWorking}
          />
          <div className="sidebar-footer">
            <SystemStatus
              allWorkspaces={workspaces}
              registry={registry}
              discrepancies={discrepancies}
              onDiscrepancyChange={refresh}
            />
            <GithubAuthFooter />
          </div>
        </aside>

        <main className="detail">
          {error && <div className="error-banner">{error}</div>}
          {registry && !registryOk && (
            <RegistryNotice registry={registry} onChanged={refresh} />
          )}
          {/*
          Mount one runner per in-flight creation so the invoke stays
          alive — and its JobEvents stay in component state — regardless
          of which pane is visible. The runner only renders its
          JobLogPane when its workspace id is the current selection.
        */}
          {Array.from(creationRuns.entries()).map(([id, args]) => (
            <CreationRunner
              key={id}
              workspaceId={id}
              args={args}
              isShown={id === selectedId}
              draftPrompt={draftPrompts.get(id) ?? ""}
              onPromptChange={(value) =>
                setDraftPrompts((prev) => {
                  const next = new Map(prev);
                  next.set(id, value);
                  return next;
                })
              }
              onSuccess={handleCreateSuccess}
              onDismiss={() => handleCreationDismiss(id)}
            />
          ))}
          {!selectedRun && selected && selected.status.kind === "ready" && (
            <WorkspaceDetail
              workspace={selected}
              session={sessionByWorkspace.get(selected.id) ?? null}
              availableRepos={
                registry?.kind === "ok"
                  ? registry.registry.repos.filter(
                      (r) =>
                        !selected.repo_links.some((l) => l.repo_key === r.key),
                    )
                  : []
              }
              notes={noteDrafts.get(selected.id) ?? selected.notes}
              onNotesChange={(value) =>
                setNoteDrafts((prev) => {
                  const next = new Map(prev);
                  next.set(selected.id, value);
                  return next;
                })
              }
              onRequestDelete={() => handleDelete(selected)}
              addRepoRun={addRepoRuns.get(selected.id) ?? null}
              onStartAddRepo={(repoKey) => startAddRepo(selected.id, repoKey)}
              onDismissAddRepo={() => dismissAddRepo(selected.id)}
            />
          )}
          {!selectedRun && !selected && registryOk && (
            <div className="placeholder">
              Select a workspace, or create one to get started.
            </div>
          )}
        </main>

        {creating && registry?.kind === "ok" && (
          <CreateWorkspaceDialog
            repos={registry.registry.repos}
            folders={folders}
            onClose={() => setCreating(false)}
            onSubmit={(partial) => {
              setCreating(false);
              // Mint the workspace id on the frontend so we can select the
              // row before the backend has even started provisioning. The
              // backend uses the same id when it inserts the Creating draft.
              const id = crypto.randomUUID();
              const args: CreateWorkspaceArgs = {
                ...partial,
                workspace_id: id,
              };
              setCreationRuns((prev) => {
                const next = new Map(prev);
                next.set(id, args);
                return next;
              });
              setSelectedId(id);
            }}
          />
        )}
      </div>
    </ThemeContext.Provider>
  );
}

/**
 * Drives one in-flight `create_workspace` invoke. Stays mounted for the
 * full lifetime of the entry in `creationRuns`, so JobEvents accumulate in
 * component state regardless of navigation; renders the JobLogPane only
 * when its workspace id is the current selection.
 */
function CreationRunner({
  workspaceId,
  args,
  isShown,
  draftPrompt,
  onPromptChange,
  onSuccess,
  onDismiss,
}: {
  workspaceId: WorkspaceId;
  args: CreateWorkspaceArgs;
  isShown: boolean;
  draftPrompt: string;
  onPromptChange: (value: string) => void;
  onSuccess: (workspaceId: WorkspaceId, result: unknown) => void;
  onDismiss: () => void;
}) {
  const descriptor = useMemo<JobDescriptor>(
    () => ({
      key: workspaceId,
      ...api.jobs.createWorkspace({ args }),
    }),
    [workspaceId, args],
  );
  const { events, state } = useBackendJob(descriptor, {
    onSuccess: (_key, result) => onSuccess(workspaceId, result),
  });
  if (!isShown) return null;
  return (
    <div className="creation-pane">
      <JobLogPane
        title={`Creating ${args.branch}`}
        events={events}
        state={state}
        onDismiss={onDismiss}
      />
      <label className="draft-prompt">
        <span className="draft-prompt-label">Initial prompt</span>
        <textarea
          autoFocus
          value={draftPrompt}
          onChange={(e) => onPromptChange(e.target.value)}
          placeholder="Write your first prompt while the workspace provisions — it'll be pasted into the session once it opens."
        />
      </label>
    </div>
  );
}

function RegistryNotice({
  registry,
  onChanged,
}: {
  registry: RegistryStatus;
  onChanged: () => void;
}) {
  const openConfig = async () => {
    try {
      await api.openConfigLocation("repos_config");
    } catch (e) {
      alert(String(e));
    }
  };

  if (registry.kind === "ok") return null;

  return (
    <div className="registry-notice">
      <h2>Repos not configured</h2>
      {registry.kind === "missing" ? (
        <p>
          Tethys expects a repo registry at <code>{registry.path}</code>. It
          doesn't exist yet.
        </p>
      ) : (
        <>
          <p>
            Tethys couldn't load <code>{registry.path}</code>:
          </p>
          <pre>{registry.error}</pre>
        </>
      )}
      <p>
        Click the button below to open it in your default editor. Fill in{" "}
        <code>worktree_root</code> and at least one <code>[[repo]]</code>, then{" "}
        <strong>restart Tethys</strong> — registry changes take effect at
        launch.
      </p>
      <div className="actions">
        <button className="primary" type="button" onClick={openConfig}>
          Open repos.toml
        </button>
        <button type="button" onClick={onChanged}>
          Re-check
        </button>
      </div>
    </div>
  );
}

function WorkspaceDetail({
  workspace,
  session,
  availableRepos,
  notes,
  onNotesChange,
  onRequestDelete,
  addRepoRun,
  onStartAddRepo,
  onDismissAddRepo,
}: {
  workspace: Workspace;
  /** The live half of the workspace's session; `null` while dormant. */
  session: SessionInfo | null;
  availableRepos: Repo[];
  /** Live notes text for this workspace — the App-level draft when there is
   *  one, else the persisted `workspace.notes`. */
  notes: string;
  onNotesChange: (notes: string) => void;
  onRequestDelete: () => void;
  /** This workspace's add-repo job, if one is running or waiting to be
   *  dismissed. Owned by `App`, so it survives leaving the workspace. */
  addRepoRun: AddRepoRun | null;
  onStartAddRepo: (repoKey: string) => void;
  onDismissAddRepo: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [showInfo, setShowInfo] = useState(false);
  const [addingRepo, setAddingRepo] = useState(false);
  const [attachingPr, setAttachingPr] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // One add-repo job per workspace at a time — the popup shows one log, and
  // the setup queue would run them one after the other regardless.
  const addRepoBusy = addRepoRun?.state === "running";
  // Sessions already auto-opened this app-run — guards against a retry loop
  // if the spawn fails, while a manual Resume click can still try again.
  const autoOpenedRef = useRef<Set<string>>(new Set());
  const prStripRef = useHorizontalScroll<HTMLDivElement>();

  const meta = workspace.session;

  // Start, Resume and Reconnect are one call: the backend reattaches,
  // resumes, or starts fresh, whichever the session's state calls for.
  const openSession = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.startAgentSession(workspace.id);
      // App-level listener on `session:changed` refreshes the cache.
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // How to name this workspace's agent in the session status copy.
  const agentLabel = workspace.agent === "codex" ? "codex" : "Claude";

  // Restart the session under another entry-point binary. History carries
  // over between binaries of the same agent; switching agent starts fresh.
  const switchBinary = async (agent: Agent, binary: string) => {
    setBusy(true);
    setError(null);
    try {
      await api.switchAgent(workspace.id, agent, binary);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // A dormant session with a saved conversation resumes without a click.
  useEffect(() => {
    if (!meta || session) return;
    if (!meta.agent_session_id) return;
    if (autoOpenedRef.current.has(meta.id)) return;
    autoOpenedRef.current.add(meta.id);
    void openSession();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [meta?.id, meta?.agent_session_id, session?.id]);

  // The backend emits `workspace:changed`, which refreshes the chip row. It
  // also records the number as dismissed, so a PR detached from the workspace's
  // own branch doesn't come back on the next poll.
  const detachPr = async (repoKey: string, prNumber: number) => {
    setError(null);
    try {
      await api.detachPr(workspace.id, repoKey, prNumber);
    } catch (e) {
      setError(String(e));
    }
  };

  const openLabel = (verb: string) =>
    busy ? (
      <>
        <Spinner /> {verb}…
      </>
    ) : (
      verb
    );

  return (
    <div className="workspace-detail">
      <div className="workspace-main">
        <header>
          <div className="header-row">
            <h2>
              <code>{workspace.branch}</code>
            </h2>
            <div className="actions">
            <BinaryMenu
              current={workspace.agent_binary ?? AGENT_CHOICES[0].binary}
              disabled={busy}
              onSwitch={switchBinary}
            />
            <button type="button" onClick={() => setShowInfo(true)}>
              Info
            </button>
            <button
              type="button"
              onClick={() => setAddingRepo(true)}
              disabled={availableRepos.length === 0 || addRepoBusy}
              title={
                addRepoBusy
                  ? "Already adding a repo to this workspace"
                  : availableRepos.length === 0
                    ? "Every repo in your registry is already in this workspace"
                    : "Add another repo's worktree to this workspace"
              }
            >
              Add repo
            </button>
            <button
              type="button"
              onClick={() =>
                api.openInVscode(workspace.id).catch((e) => setError(String(e)))
              }
              disabled={workspace.repo_links.length === 0}
              title="Open this workspace in VS Code, reusing the existing window"
            >
              Open in VS Code
            </button>
            <button
              type="button"
              className="danger"
              onClick={onRequestDelete}
              disabled={busy}
            >
              Delete
            </button>
          </div>
          </div>
          {/* The PRs get a row of their own under the title: one line that
              scrolls sideways rather than wrapping, since sharing the title
              row with the action buttons left it a few chips wide once the
              side panel took its share. "+ PR" is pinned at its end. */}
          <div className="header-prs-row">
            <div className="header-prs" ref={prStripRef}>
              {workspace.repo_links.map((r) =>
                // Skipped entirely for repos with no PRs, so the row's gap
                // doesn't double up around an empty group.
                r.prs.length > 0 ? (
                  <span className="gh-chip-group" key={r.repo_key}>
                    <RepoPrChips link={r} onDetach={detachPr} />
                  </span>
                ) : null,
              )}
            </div>
            <button
              type="button"
              className="gh-attach"
              onClick={() => setAttachingPr(true)}
              disabled={workspace.repo_links.length === 0}
              title="Track another PR in this workspace (for a second branch you opened here)"
            >
              + PR
            </button>
          </div>
        </header>
        {showInfo && (
          <WorkspaceInfoDialog
            workspace={workspace}
            onClose={() => setShowInfo(false)}
          />
        )}
        {addingRepo && (
          <AddRepoDialog
            workspace={workspace}
            availableRepos={availableRepos}
            onClose={() => setAddingRepo(false)}
            onPick={(repoKey) => {
              setAddingRepo(false);
              onStartAddRepo(repoKey);
            }}
          />
        )}
        {attachingPr && (
          <AttachPrDialog
            workspace={workspace}
            onClose={() => setAttachingPr(false)}
          />
        )}

        <div className="session-pane">
          {/* Floated over the session rather than shown above it, so an
              add-repo job can't resize the terminal on its way in and out. */}
          {addRepoRun && (
            <AddRepoPopup
              branch={workspace.branch}
              run={addRepoRun}
              onDismiss={onDismissAddRepo}
            />
          )}
          {error && <div className="error-banner">{error}</div>}
          {session ? (
            <>
              {!session.running && (
                <div className="session-exit-banner">
                  <span>{agentLabel} exited. Scrollback preserved below.</span>
                  <button
                    type="button"
                    className="primary"
                    onClick={openSession}
                    disabled={busy}
                  >
                    {meta?.agent_session_id
                      ? openLabel("Reconnect")
                      : openLabel("Start again")}
                  </button>
                </div>
              )}
              <SessionTerminal sessionId={session.id} />
            </>
          ) : meta ? (
            <div className="session-dormant">
              <p>
                This workspace's {agentLabel} session is dormant.{" "}
                {meta.agent_session_id
                  ? "Resume re-opens the saved conversation."
                  : "No conversation was saved, so Resume starts a fresh one."}
              </p>
              <button
                type="button"
                className="primary"
                onClick={openSession}
                disabled={busy}
              >
                {openLabel("Resume")}
              </button>
            </div>
          ) : (
            <div className="session-dormant">
              <p className="muted">
                No {agentLabel} session in this workspace yet.
              </p>
              <button
                type="button"
                className="primary"
                onClick={openSession}
                disabled={busy || workspace.repo_links.length === 0}
                title={
                  workspace.repo_links.length === 0
                    ? `Add a repo first — there's nowhere to run ${agentLabel}`
                    : undefined
                }
              >
                {openLabel(`Start ${agentLabel}`)}
              </button>
            </div>
          )}
        </div>
      </div>
      <SidePanel
        workspace={workspace}
        notes={notes}
        onNotesChange={onNotesChange}
      />
    </div>
  );
}

/**
 * Which agent binary the workspace's session runs under. Picking
 * another restarts the session under it, keeping the conversation when there
 * is one on disk.
 */
function BinaryMenu({
  current,
  disabled,
  onSwitch,
}: {
  current: string;
  disabled: boolean;
  onSwitch: (agent: Agent, binary: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  return (
    <div className="binary-menu-wrap" ref={wrapRef}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        disabled={disabled}
        aria-expanded={open}
        title="The agent binary this workspace's session runs under. Switching restarts the session; the conversation carries over between binaries of the same agent."
      >
        <code>{current}</code>
        <span className="caret">▾</span>
      </button>
      {open && (
        <div className="binary-menu" role="menu">
          <div className="context-menu-label">Run with</div>
          {AGENT_CHOICES.map(({ binary, agent }) => (
            <button
              key={binary}
              type="button"
              role="menuitem"
              disabled={binary === current}
              onClick={() => {
                setOpen(false);
                onSwitch(agent, binary);
              }}
            >
              {binary === current ? `${binary} ✓` : binary}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function WorkspaceInfoDialog({
  workspace,
  onClose,
}: {
  workspace: Workspace;
  onClose: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal info-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <h3>
          Workspace <code>{workspace.branch}</code>
        </h3>
        <dl className="workspace-fields">
          <dt>Created</dt>
          <dd>{new Date(workspace.created_at).toLocaleString()}</dd>
          <dt>Repos</dt>
          <dd>
            {workspace.repo_links.length === 0 ? (
              "(none)"
            ) : (
              <ul className="repo-link-list">
                {workspace.repo_links.map((r) => (
                  <li key={r.repo_key}>
                    <code>{r.repo_key}</code>
                    <span className="repo-link-path">{r.worktree_path}</span>
                    {r.setup_script_ran_at !== null && (
                      <span className="ok-badge">setup ok</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </dd>
          <dt>ID</dt>
          <dd>
            <code>{workspace.id}</code>
          </dd>
        </dl>
        <div className="modal-actions">
          <button type="button" onClick={onClose} autoFocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

/** Chips for the PRs manually attached to one repo link. */
/** Names the stack and where its tracked members sit in it. */
function stackTitle(group: PrGroup): string {
  const stack = group.stack!;
  const members = group.prs
    .map((e) => `#${e.status.pr_number} (${e.status.stack!.position})`)
    .join(" → ");
  return `Stack #${stack.number}, ${stack.size} PRs, base-first: ${members}`;
}

/**
 * One repo's PR chips: the branch PR and any attached ones, with the members of
 * a `gh stack` wrapped together in stack order. Chips stay one-per-PR — the
 * container is the only thing the grouping adds.
 */
function RepoPrChips({
  link,
  onDetach,
}: {
  link: RepoLink;
  onDetach: (repoKey: string, prNumber: number) => void;
}) {
  // Anything tracked can be detached, including the PR the poller found on its
  // own — `dismissed` on the backend is what stops the next scan re-adding it.
  const chip = ({ status, number }: LinkPr) => (
    <GithubChip
      status={status}
      onDetach={() => onDetach(link.repo_key, number)}
    />
  );

  return (
    <>
      {prGroups(linkPrEntries(link)).map((group) =>
        group.stack ? (
          <span
            key={`stack-${group.stack.number}`}
            className="gh-stack"
            title={stackTitle(group)}
          >
            {group.prs.map((entry, i) => (
              <Fragment key={entry.status.pr_number}>
                {i > 0 && (
                  <span className="gh-stack-sep" aria-hidden="true">
                    ›
                  </span>
                )}
                {chip(entry)}
              </Fragment>
            ))}
            {/* A stack whose other branches aren't checked out here would
                otherwise look like the whole thing. */}
            {group.prs.length < group.stack.size && (
              <span className="gh-stack-count">
                {group.prs.length} of {group.stack.size}
              </span>
            )}
          </span>
        ) : (
          <Fragment key={group.prs[0].status.pr_number}>
            {chip(group.prs[0])}
          </Fragment>
        ),
      )}
      {link.prs
        .filter((pr) => !pr.status)
        .map((pr) => (
          // Both paths fetch before they record — attaching by hand, and the
          // poller's follow-up pass on a PR it just discovered — so this shows
          // up only if the PR became unreachable (deleted, or GitHub is down).
          // With no status there's no stack membership, so it can't join a
          // group.
          <span
            key={pr.number}
            className="gh-chip gh-chip-missing"
            title={`PR #${pr.number} in ${link.repo_key} couldn't be fetched`}
          >
            <span className="gh-pr">{pr.number}</span>
            <span className="gh-note-badge">no data</span>
            <PrDetachButton
              prNumber={pr.number}
              onDetach={() => onDetach(link.repo_key, pr.number)}
            />
          </span>
        ))}
    </>
  );
}

/**
 * Attach a PR that the poller can't find on its own — anything on a branch
 * other than the workspace's. Accepts a PR URL or a bare number.
 */
function AttachPrDialog({
  workspace,
  onClose,
}: {
  workspace: Workspace;
  onClose: () => void;
}) {
  const [reference, setReference] = useState("");
  // `null` = let the backend infer the repo (from the URL, or because there's
  // only one candidate).
  const [repoKey, setRepoKey] = useState<string | null>(
    workspace.repo_links.length === 1 ? workspace.repo_links[0].repo_key : null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!reference.trim()) return;
    setBusy(true);
    setError(null);
    try {
      // Backend fetches the PR before persisting, so a bad number errors here.
      // Its `workspace:changed` event repaints the chip row.
      await api.attachPr(workspace.id, repoKey, reference.trim());
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={busy ? undefined : onClose}>
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <form onSubmit={submit}>
          <h3>
            Attach a PR to <code>{workspace.branch}</code>
          </h3>
          <p className="muted">
            Tethys tracks the PR for this workspace's own branch automatically.
            Attach anything you opened from a second branch here.
          </p>
          <label>
            PR
            <input
              value={reference}
              onChange={(e) => setReference(e.target.value)}
              placeholder="https://github.com/owner/repo/pull/123 or 123"
              autoFocus
            />
          </label>
          {workspace.repo_links.length > 1 && (
            <label>
              Repo
              <select
                value={repoKey ?? ""}
                onChange={(e) => setRepoKey(e.target.value || null)}
              >
                <option value="">Infer from PR URL</option>
                {workspace.repo_links.map((r) => (
                  <option key={r.repo_key} value={r.repo_key}>
                    {r.repo_key}
                  </option>
                ))}
              </select>
            </label>
          )}
          {error && <div className="error-banner">{error}</div>}
          <div className="modal-actions">
            <button type="button" onClick={onClose} disabled={busy}>
              Cancel
            </button>
            <button
              type="submit"
              className="primary"
              disabled={busy || !reference.trim()}
            >
              {busy ? (
                <>
                  <Spinner /> Attaching…
                </>
              ) : (
                "Attach"
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

/**
 * Picks the repo to add, and nothing else. The job it kicks off is owned by
 * `App` and drawn by `AddRepoPopup`, so this dialog is only ever up for as
 * long as the choice takes.
 */
function AddRepoDialog({
  workspace,
  availableRepos,
  onClose,
  onPick,
}: {
  workspace: Workspace;
  availableRepos: Repo[];
  onClose: () => void;
  onPick: (repoKey: string) => void;
}) {
  const [picked, setPicked] = useState<string | null>(null);

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!picked) return;
    onPick(picked);
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <form onSubmit={submit}>
          <h3>
            Add repo to <code>{workspace.branch}</code>
          </h3>
          {availableRepos.length === 0 ? (
            <p className="muted">
              Every repo in your registry is already in this workspace.
            </p>
          ) : (
            <div className="repo-select">
              <div className="repo-select-label">Repo</div>
              <ul>
                {availableRepos.map((r) => (
                  <li key={r.key}>
                    <label className="repo-row">
                      <input
                        type="radio"
                        name="add-repo-pick"
                        checked={picked === r.key}
                        onChange={() => setPicked(r.key)}
                      />
                      <span className="repo-display">{r.key}</span>
                    </label>
                  </li>
                ))}
              </ul>
            </div>
          )}
          <div className="modal-actions">
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            <button
              type="submit"
              className="primary"
              disabled={!picked || availableRepos.length === 0}
            >
              Add
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

/**
 * An add-repo job's log, floated over the top-right of the workspace it
 * belongs to. Deliberately not a modal: provisioning can sit in the setup
 * queue for minutes, and the session underneath it is still worth reading
 * and typing into — as are every other workspace's.
 */
function AddRepoPopup({
  branch,
  run,
  onDismiss,
}: {
  branch: string;
  run: AddRepoRun;
  onDismiss: () => void;
}) {
  return (
    <div className="add-repo-popup" role="status" aria-live="polite">
      <JobLogPane
        title={`Adding ${run.repoKey} to ${branch}`}
        events={run.events}
        state={run.state}
        onDismiss={onDismiss}
      />
    </div>
  );
}

const LAST_REPO_SELECTION_KEY = "tethys.createWorkspace.lastRepoSelection";

function loadLastRepoSelection(repos: Repo[]): Set<string> {
  const available = new Set(repos.map((r) => r.key));
  try {
    const raw = localStorage.getItem(LAST_REPO_SELECTION_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) {
        const restored = parsed.filter(
          (k): k is string => typeof k === "string" && available.has(k),
        );
        if (restored.length > 0) return new Set(restored);
      }
    }
  } catch {
    // fall through to default
  }
  return available;
}

const LAST_FOLDER_KEY = "tethys.createWorkspace.lastFolder";

/** The folder the last workspace was created into, if it's still there.
 *  Falls back to Default — which is also what an unset preference means. */
function loadLastFolder(folders: Folder[]): FolderId | null {
  try {
    const raw = localStorage.getItem(LAST_FOLDER_KEY);
    if (raw && folders.some((f) => f.id === raw)) return raw;
  } catch {
    // fall through to Default
  }
  return null;
}

/** Dialog emits everything *except* the workspace id — App mints that and
 *  merges it in before invoking. */
type CreateWorkspaceFormArgs = Omit<CreateWorkspaceArgs, "workspace_id">;

function CreateWorkspaceDialog({
  repos,
  folders,
  onClose,
  onSubmit,
}: {
  repos: Repo[];
  folders: Folder[];
  onClose: () => void;
  onSubmit: (args: CreateWorkspaceFormArgs) => void;
}) {
  const [branch, setBranch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(() =>
    loadLastRepoSelection(repos),
  );
  const [agentBinary, setAgentBinary] = useState<string>(
    AGENT_CHOICES[0].binary,
  );
  const [folder, setFolder] = useState<FolderId | null>(() =>
    loadLastFolder(folders),
  );

  const toggle = (key: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const canSubmit = branch.trim().length > 0 && selected.size > 0;

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const repoSelections = Array.from(selected);
    try {
      localStorage.setItem(
        LAST_REPO_SELECTION_KEY,
        JSON.stringify(repoSelections),
      );
      localStorage.setItem(LAST_FOLDER_KEY, folder ?? "");
    } catch {
      // non-fatal: preference just won't persist
    }
    onSubmit({
      branch: branch.trim(),
      repo_selections: repoSelections,
      agent: agentFor(agentBinary),
      agent_binary:
        agentBinary === AGENT_CHOICES[0].binary ? null : agentBinary,
      folder,
    });
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <form
        className="modal"
        onSubmit={submit}
        onClick={(e) => e.stopPropagation()}
      >
        <h3>New workspace</h3>
        <label>
          Branch
          <input
            autoFocus
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            value={branch}
            onChange={(e) => setBranch(e.target.value)}
            placeholder="e.g. ryan/session-resume"
          />
        </label>
        <div className="repo-select">
          <div className="repo-select-label">Repos</div>
          {repos.length === 0 ? (
            <p className="muted">
              No repos in registry. Add some to <code>repos.toml</code>.
            </p>
          ) : (
            <ul>
              {repos.map((r) => (
                <li key={r.key}>
                  <label className="repo-row">
                    <input
                      type="checkbox"
                      checked={selected.has(r.key)}
                      onChange={() => toggle(r.key)}
                    />
                    <span className="repo-display">{r.key}</span>
                  </label>
                </li>
              ))}
            </ul>
          )}
        </div>
        {folders.length > 0 && (
          <label>
            Folder
            <select
              value={folder ?? ""}
              onChange={(e) => setFolder(e.target.value || null)}
            >
              <option value="">Default</option>
              {folders.map((f) => (
                <option key={f.id} value={f.id}>
                  {f.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <label>
          Run with
          <select
            value={agentBinary}
            onChange={(e) => setAgentBinary(e.target.value)}
          >
            {AGENT_CHOICES.map(({ binary }) => (
              <option key={binary} value={binary}>
                {binary}
              </option>
            ))}
          </select>
        </label>
        <div className="modal-actions">
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="primary" disabled={!canSubmit}>
            Create
          </button>
        </div>
      </form>
    </div>
  );
}

function Spinner() {
  return <span className="spinner" aria-hidden="true" />;
}

export default App;
