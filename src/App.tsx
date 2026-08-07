import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  CreateWorkspaceArgs,
  Discrepancies,
  GithubPrStatus,
  GithubStatusChangedEvent,
  RegistryStatus,
  Repo,
  RepoLink,
  ScriptInfo,
  SessionInfo,
  SessionRuntimeState,
  Theme,
  TurnChangedEvent,
  Workspace,
  WorkspaceId,
} from "./types";
import { GithubAuthFooter } from "./GithubAuthFooter";
import { GithubChip, PrDetachButton } from "./GithubChip";
import { JobLogPane } from "./JobLogPane";
import { ScriptTerminal } from "./ScriptTerminal";
import { SessionTerminal } from "./SessionTerminal";
import { Sidebar } from "./Sidebar";
import { SystemStatus } from "./SystemStatus";
import { applyTheme, ThemeContext } from "./theme";
import { useBackendJob, type JobDescriptor } from "./useBackendJob";
import { useTauriEvent } from "./useTauriEvent";
import { isReadyToDelete } from "./workspaceDerived";
import "./App.css";

/** Selectable claude entry-point binaries, shared by the new-workspace form
 *  and the per-session "run with" switcher. First entry is the default. */
const CLAUDE_BINARIES = ["claude", "claude-hipaa", "claude-unsafe"] as const;

/** Bracketed-paste markers: Claude Code treats the wrapped bytes as pasted
 *  text rather than typed keystrokes, so the draft lands in the prompt box
 *  without being submitted. Mirrors the drag-drop paste in SessionTerminal. */
const PASTE_START = "\x1b[200~";
const PASTE_END = "\x1b[201~";
/** Give Claude's TUI a beat to mount its input box after the SessionStart
 *  hook fires before pasting, so the draft isn't swallowed by the startup
 *  redraw. */
const DRAFT_PROMPT_SETTLE_MS = 500;

function App() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [registry, setRegistry] = useState<RegistryStatus | null>(null);
  const [discrepancies, setDiscrepancies] = useState<Discrepancies | null>(null);
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
   * Per-session turn state tracked by listening to `session:turn_changed`
   * globally. Used for the sidebar attention dot without needing to
   * fetch sessions for every workspace. `acknowledged` mirrors the
   * persisted `turn_acknowledged` flag — the user's dismissal of the dot,
   * cleared server-side on the next runtime_state transition.
   */
  const [turnStates, setTurnStates] = useState<
    Map<
      string,
      {
        workspaceId: string;
        state: SessionRuntimeState;
        acknowledged: boolean;
      }
    >
  >(new Map());
  /**
   * Sessions for every workspace, cached so switching into a workspace
   * shows the terminal immediately instead of flashing "Dormant" during
   * the list_sessions round-trip. Populated eagerly on workspace load
   * and kept in sync via session:* events.
   */
  const [sessionsByWorkspace, setSessionsByWorkspace] = useState<
    Map<WorkspaceId, SessionInfo[]>
  >(new Map());
  /**
   * Per-workspace cache of live + recently-exited scripts. Same pattern as
   * sessions: populated on workspace load, kept in sync via `script:*`
   * events. A script can appear here with `running: false` (exited, awaiting
   * user dismissal) or `running: true`.
   */
  const [scriptsByWorkspace, setScriptsByWorkspace] = useState<
    Map<WorkspaceId, ScriptInfo[]>
  >(new Map());
  /**
   * Draft "initial prompt" text the user types while a workspace is still
   * provisioning, keyed by workspace_id. Once that workspace's first Claude
   * session reports a `claude_session_id` (its SessionStart hook fired, so the
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
    invoke<Theme | null>("get_theme")
      .then((t) => {
        setTheme(t);
        applyTheme(t);
      })
      .catch((e) => console.error("get_theme failed:", e));
  }, []);

  useTauriEvent<Theme | null>("theme:changed", (event) => {
    const t = event.payload ?? null;
    setTheme(t);
    applyTheme(t);
  });

  useTauriEvent<TurnChangedEvent>("session:turn_changed", (event) => {
    const {
      workspace_id,
      session_id,
      runtime_state,
      notification_type,
      turn_acknowledged,
    } = event.payload;
    setTurnStates((prev) => {
      const next = new Map(prev);
      if (runtime_state === "dormant") {
        next.delete(session_id);
      } else {
        next.set(session_id, {
          workspaceId: workspace_id,
          state: runtime_state,
          acknowledged: turn_acknowledged,
        });
      }
      return next;
    });
    // Keep the cached SessionInfo[] in sync so WorkspaceDetail sees the
    // new runtime_state without a full re-fetch.
    setSessionsByWorkspace((prev) => {
      const list = prev.get(workspace_id);
      if (!list) return prev;
      const next = new Map(prev);
      next.set(
        workspace_id,
        list.map((s) =>
          s.id === session_id
            ? {
                ...s,
                runtime_state,
                notification_type: notification_type ?? null,
                turn_acknowledged,
              }
            : s,
        ),
      );
      return next;
    });
  });

  useTauriEvent<GithubStatusChangedEvent>("github:status_changed", (event) => {
    const { workspace_id, repo_key, pr_number, status } = event.payload;
    setWorkspaces((prev) =>
      prev.map((w) => {
        if (w.id !== workspace_id) return w;
        return {
          ...w,
          repo_links: w.repo_links.map((r) => {
            if (r.repo_key !== repo_key) return r;
            // A pr_number means the update is for a manually-attached PR,
            // which lives in its own slot alongside the branch PR.
            if (pr_number === null) return { ...r, github: status };
            return {
              ...r,
              attached_prs: r.attached_prs.map((a) =>
                a.number === pr_number ? { ...a, status } : a,
              ),
            };
          }),
        };
      }),
    );
  });

  const workspaceNeedsTurn = useCallback(
    (w: Workspace): boolean => {
      if (w.archived_at) return false;
      for (const info of turnStates.values()) {
        if (info.workspaceId !== w.id) continue;
        if (info.state !== "idle" && info.state !== "waiting_input") continue;
        if (info.acknowledged) continue;
        return true;
      }
      return false;
    },
    [turnStates],
  );

  const workspaceWorking = useCallback(
    (w: Workspace): boolean => {
      if (w.archived_at) return false;
      for (const info of turnStates.values()) {
        if (info.workspaceId !== w.id) continue;
        if (info.state === "working") return true;
      }
      return false;
    },
    [turnStates],
  );

  const runningScriptNamesFor = useCallback(
    (w: Workspace): string[] => {
      const scripts = scriptsByWorkspace.get(w.id);
      if (!scripts) return [];
      return scripts.filter((s) => s.running).map((s) => s.script_name);
    },
    [scriptsByWorkspace],
  );

  const handleClearTurn = useCallback(
    (workspace: Workspace) => {
      // Backend persists turn_acknowledged + emits session:turn_changed
      // back, which updates turnStates. No optimistic local update needed —
      // the round-trip is fast and the persisted flag is the source of truth.
      for (const [sessionId, info] of turnStates) {
        if (info.workspaceId !== workspace.id) continue;
        if (info.state !== "idle" && info.state !== "waiting_input") continue;
        if (info.acknowledged) continue;
        invoke("acknowledge_session_turn", {
          workspaceId: workspace.id,
          sessionId,
        }).catch((e) =>
          console.error("acknowledge_session_turn failed:", e),
        );
      }
    },
    [turnStates],
  );

  const refreshSessionsFor = useCallback(async (workspaceId: WorkspaceId) => {
    try {
      const list = await invoke<SessionInfo[]>("list_sessions", {
        workspaceId,
      });
      setSessionsByWorkspace((prev) => {
        const next = new Map(prev);
        next.set(workspaceId, list);
        return next;
      });
      // Seed turnStates from the listing. The backend restores
      // runtime_state from disk on boot via seed_turn but intentionally
      // doesn't emit session:turn_changed (the frontend isn't subscribed
      // yet) — without this, the sidebar dot stays dark across restarts
      // until the next live event fires for that session.
      setTurnStates((prev) => {
        let next: Map<
          string,
          {
            workspaceId: string;
            state: SessionRuntimeState;
            acknowledged: boolean;
          }
        > | null = null;
        const ensure = () => {
          if (!next) next = new Map(prev);
          return next;
        };
        for (const s of list) {
          if (s.runtime_state === "dormant") {
            if (prev.has(s.id)) ensure().delete(s.id);
            continue;
          }
          const cur = prev.get(s.id);
          if (
            !cur ||
            cur.state !== s.runtime_state ||
            cur.workspaceId !== workspaceId ||
            cur.acknowledged !== s.turn_acknowledged
          ) {
            ensure().set(s.id, {
              workspaceId,
              state: s.runtime_state,
              acknowledged: s.turn_acknowledged,
            });
          }
        }
        return next ?? prev;
      });
    } catch (e) {
      console.error("list_sessions:", e);
    }
  }, []);

  const refreshScriptsFor = useCallback(async (workspaceId: WorkspaceId) => {
    try {
      const list = await invoke<ScriptInfo[]>("list_scripts", {
        workspaceId,
      });
      setScriptsByWorkspace((prev) => {
        const next = new Map(prev);
        next.set(workspaceId, list);
        return next;
      });
    } catch (e) {
      console.error("list_scripts:", e);
    }
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [list, reg, disc] = await Promise.all([
        invoke<Workspace[]>("list_workspaces"),
        invoke<RegistryStatus>("registry_status"),
        invoke<Discrepancies>("list_discrepancies"),
      ]);
      setWorkspaces(list);
      setRegistry(reg);
      setDiscrepancies(disc);
      setError(null);
      // Pre-load sessions + scripts for every workspace so switching in
      // doesn't render a stale/empty list.
      await Promise.all(
        list.flatMap((w) => [refreshSessionsFor(w.id), refreshScriptsFor(w.id)]),
      );
    } catch (e) {
      setError(String(e));
    }
  }, [refreshSessionsFor, refreshScriptsFor]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useTauriEvent("workspace:changed", () => refresh());
  useTauriEvent<{ workspace_id: string }>("session:changed", (event) => {
    refreshSessionsFor(event.payload.workspace_id);
  });
  useTauriEvent<{ workspace_id: string }>("session:exit", (event) => {
    refreshSessionsFor(event.payload.workspace_id);
  });
  useTauriEvent<{ workspace_id: string }>("script:changed", (event) => {
    refreshScriptsFor(event.payload.workspace_id);
  });
  useTauriEvent<{ workspace_id: string }>("script:exit", (event) => {
    refreshScriptsFor(event.payload.workspace_id);
  });

  // Paste any draft initial-prompt into a workspace's first Claude session
  // once it's up. `workspace:changed` fires (and refreshes `workspaces`) when
  // the SessionStart hook populates `claude_session_id`, which is our signal
  // that the TUI is ready to receive a paste.
  useEffect(() => {
    for (const [workspaceId, prompt] of draftPrompts) {
      if (flushedDraftsRef.current.has(workspaceId)) continue;
      if (prompt.trim().length === 0) continue;
      const ws = workspaces.find((w) => w.id === workspaceId);
      if (!ws || ws.status.kind !== "ready") continue;
      const session = ws.sessions.find((s) => s.claude_session_id !== null);
      if (!session) continue;

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
          await invoke("send_input", { sessionId, data: bytes });
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
  // Navigable list for hotkeys: same set the sidebar's main "active"
  // section shows (drops archived). Order matches the sidebar.
  const navigableWorkspaces = useMemo(
    () => visibleWorkspaces.filter((w) => !w.archived_at),
    [visibleWorkspaces],
  );

  // Keep the latest values reachable from a stable keydown handler so we
  // don't re-bind (and tear down) the window listener on every render.
  const navRef = useRef({
    list: navigableWorkspaces,
    selectedId,
    needsTurn: workspaceNeedsTurn,
    workspaces,
    clearTurn: handleClearTurn,
  });
  navRef.current = {
    list: navigableWorkspaces,
    selectedId,
    needsTurn: workspaceNeedsTurn,
    workspaces,
    clearTurn: handleClearTurn,
  };

  useEffect(() => {
    const step = (direction: 1 | -1, attentionOnly: boolean) => {
      const { list, selectedId: cur, needsTurn } = navRef.current;
      const pool = attentionOnly ? list.filter((w) => needsTurn(w)) : list;
      if (pool.length === 0) return;
      // Find the anchor inside the pool. When attentionOnly and the
      // current selection has no dot, anchor by its position in the full
      // list so direction still feels right.
      let anchor = pool.findIndex((w) => w.id === cur);
      if (anchor === -1 && attentionOnly && cur) {
        const fullIdx = list.findIndex((w) => w.id === cur);
        if (fullIdx !== -1) {
          // Walk in `direction` until we hit a pool member.
          for (
            let i = direction === 1 ? fullIdx + 1 : fullIdx - 1;
            i >= 0 && i < list.length;
            i += direction
          ) {
            const hit = pool.findIndex((w) => w.id === list[i].id);
            if (hit !== -1) {
              setSelectedId(pool[hit].id);
              return;
            }
          }
          // Nothing in that direction — wrap.
          setSelectedId(
            direction === 1 ? pool[0].id : pool[pool.length - 1].id,
          );
          return;
        }
      }
      if (anchor === -1) anchor = direction === 1 ? -1 : 0;
      const next = (anchor + direction + pool.length) % pool.length;
      setSelectedId(pool[next].id);
    };

    const handler = (e: KeyboardEvent) => {
      // Cmd+Alt(+Shift) + J/K to navigate; Cmd+Alt+. to clear the
      // current workspace's "your turn" dot. `e.code` ignores Option's
      // character remapping on macOS (Alt+J → ˝), so bindings survive layout.
      if (!e.metaKey || !e.altKey || e.ctrlKey) return;
      if (e.code === "KeyJ" || e.code === "KeyK") {
        const direction: 1 | -1 = e.code === "KeyK" ? 1 : -1;
        e.preventDefault();
        e.stopPropagation();
        step(direction, e.shiftKey);
        return;
      }
      if (e.code === "Period") {
        const { selectedId: cur, workspaces, clearTurn } = navRef.current;
        if (!cur) return;
        const ws = workspaces.find((w) => w.id === cur);
        if (!ws) return;
        e.preventDefault();
        e.stopPropagation();
        clearTurn(ws);
      }
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () =>
      window.removeEventListener("keydown", handler, { capture: true });
  }, []);

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
      // Auto-start a Claude session: in the only repo when the workspace
      // has just one, otherwise at the workspace root.
      const repoKey =
        ws.repo_links.length === 1 ? ws.repo_links[0].repo_key : null;
      try {
        await invoke<SessionInfo>("start_claude_session", {
          args: { workspace_id: ws.id, repo_key: repoKey },
        });
      } catch (e) {
        setError(`auto-start claude failed: ${String(e)}`);
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
        await invoke("forget_workspace", { id: workspaceId });
      } catch (e) {
        // Workspace may already be gone (e.g. invoke rejected before the
        // draft was even inserted) — not fatal, just log and move on.
        console.warn("forget_workspace failed:", e);
      }
    },
    [],
  );

  const handleDelete = useCallback(async (workspace: Workspace) => {
    setSelectedId((cur) => (cur === workspace.id ? null : cur));
    // CreationFailed entries have no worktrees on disk, so skip the
    // soft-delete + 1-hour grace window and just drop from state.
    const command =
      workspace.status.kind === "creation_failed"
        ? "forget_workspace"
        : "delete_workspace";
    try {
      await invoke(command, { id: workspace.id });
    } catch (e) {
      setError(`delete failed: ${String(e)}`);
    }
  }, []);

  const handleArchiveToggle = useCallback(async (workspace: Workspace) => {
    try {
      await invoke(
        workspace.archived_at ? "unarchive_workspace" : "archive_workspace",
        { id: workspace.id },
      );
    } catch (e) {
      setError(`archive failed: ${String(e)}`);
    }
  }, []);

  const handleReorder = useCallback(async (ids: WorkspaceId[]) => {
    // Optimistically reorder so the drop animation lands on the right row,
    // then fire the backend command. The `workspace:reordered` event also
    // drives a refresh — doing this here avoids the round-trip flicker.
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
      await invoke("reorder_workspaces", { ids });
    } catch (e) {
      setError(`reorder failed: ${String(e)}`);
    }
  }, []);

  const registryOk = registry?.kind === "ok";
  const selectedRun = selectedId ? creationRuns.get(selectedId) ?? null : null;

  return (
    <ThemeContext.Provider value={theme}>
    <div className="app">
      <aside className="sidebar">
        <div className="sidebar-header">
          <button
            className="primary"
            onClick={() => setCreating(true)}
            type="button"
            disabled={!registryOk}
            title={!registryOk ? "Configure repos.toml first" : undefined}
          >
            New workspace
          </button>
        </div>
        <Sidebar
          workspaces={visibleWorkspaces}
          selectedId={selectedId}
          onSelect={setSelectedId}
          onReorder={handleReorder}
          onArchiveToggle={handleArchiveToggle}
          onDelete={handleDelete}
          onClearTurn={handleClearTurn}
          workspaceNeedsTurn={workspaceNeedsTurn}
          workspaceWorking={workspaceWorking}
          runningScriptNames={runningScriptNamesFor}
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
            sessions={sessionsByWorkspace.get(selected.id) ?? []}
            scripts={scriptsByWorkspace.get(selected.id) ?? []}
            registryRepos={
              registry?.kind === "ok" ? registry.registry.repos : []
            }
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
            onRequestArchive={() => handleArchiveToggle(selected)}
            onRepoAdded={refresh}
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
          onClose={() => setCreating(false)}
          onSubmit={(partial) => {
            setCreating(false);
            // Mint the workspace id on the frontend so we can select the
            // row before the backend has even started provisioning. The
            // backend uses the same id when it inserts the Creating draft.
            const id = crypto.randomUUID();
            const args: CreateWorkspaceArgs = { ...partial, workspace_id: id };
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
      command: "create_workspace",
      args: { args },
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
          placeholder="Write your first prompt while the workspace provisions — it'll be pasted into Claude once the session opens."
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
      await invoke("open_repos_config");
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

/**
 * Discriminated tab key for the workspace's main pane. Sessions and scripts
 * share the chip bar — exactly one is "selected" at a time. The script
 * variant is keyed by `(repoKey, scriptName)` rather than the run id, so the
 * tab survives stop/start cycles (a new run gets a fresh id).
 */
type SelectedTab =
  | { kind: "session"; metaId: string }
  | { kind: "script"; repoKey: string; scriptName: string };

function scriptTabKey(repoKey: string, scriptName: string): string {
  return `script:${repoKey}:${scriptName}`;
}

/** Collapsible freeform notes overlay anchored to the top-right of a
 *  workspace's detail pane. Edits are debounced to `set_workspace_notes` and
 *  flushed on collapse/unmount so nothing is lost when switching workspaces.
 *  Keyed by workspace id at the call site so each workspace gets a fresh
 *  editor; the text itself lives in App's `noteDrafts` so it survives that
 *  remount (see the state's doc comment). */
function WorkspaceNotes({
  workspaceId,
  notes,
  onNotesChange,
}: {
  workspaceId: string;
  notes: string;
  onNotesChange: (notes: string) => void;
}) {
  // Auto-open for any workspace that already has notes, so switching in
  // surfaces them without a click. Collapsing sticks until the next switch
  // (the remount re-evaluates this seed).
  const [open, setOpen] = useState(() => notes.trim().length > 0);
  // Only steal focus when the user opened the panel themselves — an auto-open
  // on workspace switch must leave the keyboard with the terminal.
  const openedByUser = useRef(false);
  const saveTimer = useRef<number | null>(null);
  // Latest unsaved value, or null once it's been persisted. Lets the flush on
  // unmount/collapse write the final keystrokes the debounce hasn't sent yet.
  const pending = useRef<string | null>(null);

  const save = useCallback(
    (notes: string) => {
      pending.current = null;
      invoke("set_workspace_notes", {
        args: { workspace_id: workspaceId, notes },
      }).catch(() => {
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

  // Flush any pending edit when the editor unmounts (e.g. switching workspace).
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

  if (!open) {
    return (
      <button
        type="button"
        className="notes-toggle"
        onClick={() => {
          openedByUser.current = true;
          setOpen(true);
        }}
        title="Workspace notes"
      >
        {notes.trim() ? "Notes •" : "Notes"}
      </button>
    );
  }

  return (
    <div className="notes-panel" role="dialog" aria-label="Workspace notes">
      <div className="notes-panel-header">
        <span>Notes</span>
        <button
          type="button"
          className="notes-collapse"
          onClick={() => {
            flush();
            setOpen(false);
          }}
          title="Collapse notes"
        >
          ✕
        </button>
      </div>
      <textarea
        className="notes-textarea"
        value={notes}
        placeholder="Jot down anything about this workspace…"
        onChange={(e) => onChange(e.target.value)}
        autoFocus={openedByUser.current}
      />
    </div>
  );
}

function WorkspaceDetail({
  workspace,
  sessions,
  scripts,
  registryRepos,
  availableRepos,
  notes,
  onNotesChange,
  onRequestDelete,
  onRequestArchive,
  onRepoAdded,
}: {
  workspace: Workspace;
  sessions: SessionInfo[];
  scripts: ScriptInfo[];
  /** Every repo in the registry — used to look up configured scripts for the
   *  workspace's linked repos. */
  registryRepos: Repo[];
  availableRepos: Repo[];
  /** Live notes text for this workspace — the App-level draft when there is
   *  one, else the persisted `workspace.notes`. */
  notes: string;
  onNotesChange: (notes: string) => void;
  onRequestDelete: () => void;
  onRequestArchive: () => void;
  onRepoAdded: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [showInfo, setShowInfo] = useState(false);
  const [addingRepo, setAddingRepo] = useState(false);
  const [attachingPr, setAttachingPr] = useState(false);
  // Per-workspace selection. Derived on render (no effect), so switching
  // back to a workspace paints the remembered pick immediately.
  const [selectedByWorkspace, setSelectedByWorkspace] = useState<
    Map<string, SelectedTab>
  >(new Map());
  const selectedTab = selectedByWorkspace.get(workspace.id) ?? null;
  const selectedSessionId =
    selectedTab?.kind === "session" ? selectedTab.metaId : null;
  const setSelectedTab = (tab: SelectedTab | null) => {
    setSelectedByWorkspace((prev) => {
      const next = new Map(prev);
      if (tab) next.set(workspace.id, tab);
      else next.delete(workspace.id);
      return next;
    });
  };
  const selectSession = (id: string | null) => {
    setSelectedTab(id ? { kind: "session", metaId: id } : null);
  };
  const selectScript = (repoKey: string, scriptName: string) => {
    setSelectedTab({ kind: "script", repoKey, scriptName });
  };
  const [error, setError] = useState<string | null>(null);
  // Meta ids we've already auto-resumed this app-run — guards against
  // retry loops if spawn fails, while still allowing a manual Resume
  // click to try again.
  const autoResumedRef = useRef<Set<string>>(new Set());

  const liveById = new Map(sessions.map((s) => [s.id, s]));
  // `workspace.sessions` is append-ordered on the backend, so the most
  // recently started session sits last — i.e. on the right of the tab row.
  const ordered = [...workspace.sessions];
  const visibleOrdered = ordered.filter((m) => !m.hidden);
  const hiddenOrdered = ordered.filter((m) => m.hidden);
  const [showHidden, setShowHidden] = useState(false);

  // Build script chips: every configured (repo, script_name) becomes one
  // chip. If there's a running/exited ScriptInfo for that pair it gets
  // attached; otherwise the chip renders as idle.
  const scriptChips: ScriptChipData[] = [];
  for (const link of workspace.repo_links) {
    const repo = registryRepos.find((r) => r.key === link.repo_key);
    if (!repo) continue;
    for (const [name, command] of Object.entries(repo.scripts ?? {})) {
      const run =
        scripts.find(
          (s) => s.repo_key === link.repo_key && s.script_name === name,
        ) ?? null;
      scriptChips.push({
        repoKey: link.repo_key,
        scriptName: name,
        command,
        run,
      });
    }
  }

  const selectedScriptChip =
    selectedTab?.kind === "script"
      ? scriptChips.find(
          (c) =>
            c.repoKey === selectedTab.repoKey &&
            c.scriptName === selectedTab.scriptName,
        ) ?? null
      : null;

  // Effective session selection: only used when a session tab (or nothing)
  // is selected. Prefer the user's explicit pick when it's still visible,
  // else newest live, else newest. Newest is last in append order.
  const candidates = showHidden ? ordered : visibleOrdered;
  const effectiveSelected = (() => {
    if (selectedTab?.kind === "script") return null;
    if (selectedSessionId && candidates.some((m) => m.id === selectedSessionId))
      return selectedSessionId;
    const lastLive = [...candidates].reverse().find((m) => liveById.has(m.id));
    return lastLive?.id ?? candidates[candidates.length - 1]?.id ?? null;
  })();

  const selected = effectiveSelected
    ? ordered.find((m) => m.id === effectiveSelected) ?? null
    : null;
  const selectedLive = selected ? liveById.get(selected.id) ?? null : null;

  // `repoKey === null` spawns at the workspace root (parent of every
  // repo worktree) — only offered when the workspace has 2+ repos.
  const startInRepo = async (repoKey: string | null) => {
    setBusy(true);
    setError(null);
    try {
      const res = await invoke<SessionInfo>("start_claude_session", {
        args: { workspace_id: workspace.id, repo_key: repoKey },
      });
      selectSession(res.id);
      // App-level listener on `session:changed` refreshes the cache.
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Acknowledge a single session's "your turn" indicator. The backend
  // persists turn_acknowledged + emits session:turn_changed, which clears
  // the chip dot (and folds into the workspace row aggregate).
  const clearSessionTurn = (sessionId: string) => {
    invoke("acknowledge_session_turn", {
      workspaceId: workspace.id,
      sessionId,
    }).catch((e) => console.error("acknowledge_session_turn failed:", e));
  };

  const setSessionHidden = async (sessionId: string, hidden: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_claude_session_hidden", {
        args: { workspace_id: workspace.id, session_id: sessionId, hidden },
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const resumeMeta = async (metaId: string, repoKey: string | null) => {
    setBusy(true);
    setError(null);
    try {
      const res = await invoke<SessionInfo>("resume_claude_session", {
        args: {
          workspace_id: workspace.id,
          repo_key: repoKey,
          session_meta_id: metaId,
        },
      });
      selectSession(res.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Switch the entry-point binary for an in-progress chat. Restarts the
  // session under the new binary via `claude --resume`, keeping history.
  const switchBinary = async (metaId: string, binary: string) => {
    setBusy(true);
    setError(null);
    try {
      const res = await invoke<SessionInfo>("switch_claude_binary", {
        args: {
          workspace_id: workspace.id,
          session_meta_id: metaId,
          claude_binary: binary,
        },
      });
      selectSession(res.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Auto-resume the selected session when it's dormant but has a
  // claude_session_id. `autoResumedRef` prevents a retry loop if the
  // spawn fails — the user can still click Resume manually below.
  useEffect(() => {
    if (selectedTab?.kind === "script") return;
    if (!selected || selectedLive) return;
    if (!selected.claude_session_id) return;
    if (autoResumedRef.current.has(selected.id)) return;
    autoResumedRef.current.add(selected.id);
    void resumeMeta(selected.id, selected.repo_key);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.id, selectedLive?.id, selected?.claude_session_id, selectedTab?.kind]);

  const startScript = async (repoKey: string, scriptName: string) => {
    setBusy(true);
    setError(null);
    try {
      await invoke<ScriptInfo>("start_script", {
        args: {
          workspace_id: workspace.id,
          repo_key: repoKey,
          script_name: scriptName,
        },
      });
      selectScript(repoKey, scriptName);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const dismissScript = async (scriptId: string) => {
    try {
      await invoke("dismiss_script", {
        args: { workspace_id: workspace.id, script_id: scriptId },
      });
    } catch (e) {
      setError(String(e));
    }
  };

  // The backend emits `workspace:changed`, which refreshes the chip row.
  const detachPr = async (repoKey: string, prNumber: number) => {
    setError(null);
    try {
      await invoke("detach_pr", {
        args: {
          workspace_id: workspace.id,
          repo_key: repoKey,
          pr_number: prNumber,
        },
      });
    } catch (e) {
      setError(String(e));
    }
  };

  const handleScriptChipClick = (chip: ScriptChipData) => {
    if (!chip.run) {
      void startScript(chip.repoKey, chip.scriptName);
      return;
    }
    selectScript(chip.repoKey, chip.scriptName);
  };

  return (
    <div className="workspace-detail">
      <header>
        <h2>
          <code>{workspace.branch}</code>
          {workspace.repo_links.map((r) =>
            // Skipped entirely for repos with no PRs, so the header's gap
            // doesn't double up around an empty group.
            r.github || r.attached_prs.length > 0 ? (
              <span className="gh-chip-group" key={r.repo_key}>
                {r.github && <GithubChip status={r.github} />}
                <AttachedPrChips link={r} onDetach={detachPr} />
              </span>
            ) : null,
          )}
          <button
            type="button"
            className="gh-attach"
            onClick={() => setAttachingPr(true)}
            disabled={workspace.repo_links.length === 0}
            title="Track another PR in this workspace (for a second branch you opened here)"
          >
            + PR
          </button>
        </h2>
        <div className="actions">
          <button type="button" onClick={() => setShowInfo(true)}>
            Info
          </button>
          <button
            type="button"
            onClick={() => setAddingRepo(true)}
            disabled={availableRepos.length === 0}
            title={
              availableRepos.length === 0
                ? "Every repo in your registry is already in this workspace"
                : "Add another repo's worktree to this workspace"
            }
          >
            Add repo
          </button>
          <button
            type="button"
            onClick={() =>
              invoke("open_in_vscode", { id: workspace.id }).catch((e) =>
                setError(String(e)),
              )
            }
            disabled={workspace.repo_links.length === 0}
            title="Open every worktree in VS Code"
          >
            Open in VS Code
          </button>
          <button
            type="button"
            onClick={onRequestArchive}
            disabled={busy}
          >
            {workspace.archived_at ? "Unarchive" : "Archive"}
          </button>
          <button
            type="button"
            className="danger"
            onClick={onRequestDelete}
            disabled={busy}
          >
            Delete
          </button>
          <WorkspaceNotes
            key={workspace.id}
            workspaceId={workspace.id}
            notes={notes}
            onNotesChange={onNotesChange}
          />
        </div>
      </header>
      {!workspace.archived_at && isReadyToDelete(workspace) && (
        <div className="archive-banner">
          <div>
            <strong>Ready to delete.</strong>{" "}
            <span className="muted">
              Every linked PR for <code>{workspace.branch}</code> is merged.
            </span>
          </div>
          <button
            type="button"
            className="primary"
            onClick={onRequestDelete}
          >
            Delete workspace
          </button>
        </div>
      )}
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
          onSuccess={onRepoAdded}
        />
      )}
      {attachingPr && (
        <AttachPrDialog
          workspace={workspace}
          onClose={() => setAttachingPr(false)}
        />
      )}

      <div className="session-pane">
        <SessionBar
          visibleSessions={visibleOrdered}
          hiddenSessions={hiddenOrdered}
          showHidden={showHidden}
          onToggleShowHidden={() => setShowHidden((v) => !v)}
          liveById={liveById}
          repos={workspace.repo_links}
          selectedId={effectiveSelected}
          onSelect={selectSession}
          onStartInRepo={startInRepo}
          onSetHidden={setSessionHidden}
          onClearTurn={clearSessionTurn}
          workspaceBinary={workspace.claude_binary}
          onSwitchBinary={switchBinary}
          scriptChips={scriptChips}
          selectedScriptKey={
            selectedTab?.kind === "script"
              ? scriptTabKey(selectedTab.repoKey, selectedTab.scriptName)
              : null
          }
          showRepoOnScript={workspace.repo_links.length > 1}
          onScriptChipClick={handleScriptChipClick}
          onScriptChipDismiss={dismissScript}
          busy={busy}
        />
        {error && <div className="error-banner">{error}</div>}
        {selectedScriptChip ? (
          selectedScriptChip.run ? (
            <>
              {!selectedScriptChip.run.running && (
                <div className="session-exit-banner">
                  <span>
                    Script <code>{selectedScriptChip.scriptName}</code> exited.
                  </span>
                  <button
                    type="button"
                    className="primary"
                    onClick={() =>
                      startScript(
                        selectedScriptChip.repoKey,
                        selectedScriptChip.scriptName,
                      )
                    }
                    disabled={busy}
                  >
                    Restart
                  </button>
                </div>
              )}
              <ScriptTerminal scriptId={selectedScriptChip.run.id} />
            </>
          ) : (
            <div className="session-dormant">
              <p>
                Script <code>{selectedScriptChip.scriptName}</code> is not
                running.
              </p>
              <p className="muted">
                <code>{selectedScriptChip.command}</code>
              </p>
              <button
                type="button"
                className="primary"
                onClick={() =>
                  startScript(
                    selectedScriptChip.repoKey,
                    selectedScriptChip.scriptName,
                  )
                }
                disabled={busy}
              >
                {busy ? (
                  <>
                    <Spinner /> Starting…
                  </>
                ) : (
                  "Start"
                )}
              </button>
            </div>
          )
        ) : selected ? (
          selectedLive ? (
            <>
              {!selectedLive.running && (
                <div className="session-exit-banner">
                  <span>Claude exited. Scrollback preserved below.</span>
                  {selected.claude_session_id ? (
                    <button
                      type="button"
                      className="primary"
                      onClick={() =>
                        resumeMeta(selected.id, selected.repo_key)
                      }
                      disabled={busy}
                    >
                      {busy ? (
                        <>
                          <Spinner /> Reconnecting…
                        </>
                      ) : (
                        "Reconnect"
                      )}
                    </button>
                  ) : (
                    <span className="muted">
                      No claude_session_id — can't reconnect.
                    </span>
                  )}
                </div>
              )}
              <SessionTerminal sessionId={selectedLive.id} />
            </>
          ) : (
            <div className="session-dormant">
              <p>
                This Claude session is dormant. Resume re-opens the
                conversation with <code>claude --resume</code>.
              </p>
              {selected.claude_session_id ? (
                <button
                  type="button"
                  className="primary"
                  onClick={() => resumeMeta(selected.id, selected.repo_key)}
                  disabled={busy}
                >
                  {busy ? (
                    <>
                      <Spinner /> Resuming…
                    </>
                  ) : (
                    "Resume"
                  )}
                </button>
              ) : (
                <p className="muted">
                  No <code>claude_session_id</code> was captured for this
                  session — can't resume. (If you just started it, wait a
                  second for the SessionStart hook.)
                </p>
              )}
            </div>
          )
        ) : (
          <div className="session-pane empty">
            <p className="muted">No Claude sessions in this workspace yet.</p>
            <p className="muted">
              Click <strong>+ New</strong> above to start one.
            </p>
          </div>
        )}
      </div>
    </div>
  );
}

type ChipMeta = {
  id: string;
  repo_key: string | null;
  claude_session_id: string | null;
  claude_binary: string | null;
};

type ScriptChipData = {
  repoKey: string;
  scriptName: string;
  command: string;
  /** `null` => not running and no stale handle to attach to. */
  run: ScriptInfo | null;
};

function SessionChip({
  meta,
  hidden,
  selected,
  live,
  onSelect,
  onSetHidden,
  onContextMenu,
}: {
  meta: ChipMeta;
  hidden: boolean;
  selected: boolean;
  live: SessionInfo | undefined;
  onSelect: (id: string) => void;
  onSetHidden: (id: string, hidden: boolean) => void;
  onContextMenu: (id: string, x: number, y: number) => void;
}) {
  const label = meta.id.slice(0, 8);
  const needsTurn =
    live?.running &&
    !live.turn_acknowledged &&
    (live.runtime_state === "idle" || live.runtime_state === "waiting_input");
  const chipClass = [
    "session-chip",
    selected ? "active" : "",
    hidden ? "hidden" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div
      role="button"
      tabIndex={0}
      className={chipClass}
      onClick={() => onSelect(meta.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        onContextMenu(meta.id, e.clientX, e.clientY);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect(meta.id);
        }
      }}
    >
      <span className={`chip-repo${meta.repo_key === null ? " root" : ""}`}>
        {meta.repo_key ?? "root"}
      </span>
      <code>{label}</code>
      {needsTurn && <span className="turn-dot" />}
      {live && !live.running && <span className="chip-state">exited</span>}
      {!live && <span className="chip-state">dormant</span>}
      <button
        type="button"
        className="chip-x"
        title={hidden ? "Show this chat" : "Hide this chat"}
        aria-label={hidden ? "Show this chat" : "Hide this chat"}
        onClick={(e) => {
          e.stopPropagation();
          onSetHidden(meta.id, !hidden);
        }}
      >
        {hidden ? "↺" : "×"}
      </button>
    </div>
  );
}

function ScriptChip({
  chip,
  showRepo,
  selected,
  onClick,
  onDismiss,
}: {
  chip: ScriptChipData;
  showRepo: boolean;
  selected: boolean;
  onClick: (chip: ScriptChipData) => void;
  onDismiss: (scriptId: string) => void;
}) {
  const running = chip.run?.running ?? false;
  const exited = chip.run !== null && !chip.run.running;
  const idle = chip.run === null;
  const indicator = running ? "▶" : exited ? "■" : "○";
  const stateClass = running ? "running" : exited ? "exited" : "idle";
  return (
    <div
      role="button"
      tabIndex={0}
      className={`session-chip script-chip ${stateClass}${selected ? " active" : ""}`}
      onClick={() => onClick(chip)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick(chip);
        }
      }}
      title={chip.command}
    >
      <span className="script-indicator" aria-hidden="true">
        {indicator}
      </span>
      {showRepo && <span className="chip-repo">{chip.repoKey}</span>}
      <code>{chip.scriptName}</code>
      {!idle && chip.run && (
        <button
          type="button"
          className="chip-x"
          title={running ? "Cancel this script" : "Dismiss exited logs"}
          aria-label={running ? "Cancel this script" : "Dismiss exited logs"}
          onClick={(e) => {
            e.stopPropagation();
            onDismiss(chip.run!.id);
          }}
        >
          ×
        </button>
      )}
    </div>
  );
}

function SessionBar({
  visibleSessions,
  hiddenSessions,
  showHidden,
  onToggleShowHidden,
  liveById,
  repos,
  selectedId,
  onSelect,
  onStartInRepo,
  onSetHidden,
  onClearTurn,
  workspaceBinary,
  onSwitchBinary,
  scriptChips,
  selectedScriptKey,
  showRepoOnScript,
  onScriptChipClick,
  onScriptChipDismiss,
  busy,
}: {
  visibleSessions: ChipMeta[];
  hiddenSessions: ChipMeta[];
  showHidden: boolean;
  onToggleShowHidden: () => void;
  liveById: Map<string, SessionInfo>;
  repos: { repo_key: string }[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  /** `null` => start at the workspace root. */
  onStartInRepo: (repoKey: string | null) => void;
  onSetHidden: (id: string, hidden: boolean) => void;
  onClearTurn: (id: string) => void;
  /** Workspace-level binary default; the fallback when a session has no
   *  per-session override. `null` => the app default `claude`. */
  workspaceBinary: string | null;
  onSwitchBinary: (id: string, binary: string) => void;
  scriptChips: ScriptChipData[];
  selectedScriptKey: string | null;
  /** Show the repo prefix on the script chip — only useful when the
   *  workspace has 2+ repos, otherwise the prefix is noise. */
  showRepoOnScript: boolean;
  onScriptChipClick: (chip: ScriptChipData) => void;
  onScriptChipDismiss: (scriptId: string) => void;
  busy: boolean;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  // Right-click context menu for a single session chip.
  const [chipMenu, setChipMenu] = useState<{
    sessionId: string;
    x: number;
    y: number;
  } | null>(null);
  const openChipMenu = (sessionId: string, x: number, y: number) =>
    setChipMenu({ sessionId, x, y });

  // Close the "+ New" repo menu on outside click.
  useEffect(() => {
    if (!menuOpen) return;
    const handler = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [menuOpen]);

  const onNewClick = () => {
    if (repos.length === 0) return;
    if (repos.length === 1) {
      onStartInRepo(repos[0].repo_key);
      return;
    }
    setMenuOpen((v) => !v);
  };

  return (
    <div className="session-chip-bar">
      {visibleSessions.map((m) => (
        <SessionChip
          key={m.id}
          meta={m}
          hidden={false}
          selected={selectedId === m.id}
          live={liveById.get(m.id)}
          onSelect={onSelect}
          onSetHidden={onSetHidden}
          onContextMenu={openChipMenu}
        />
      ))}
      {showHidden &&
        hiddenSessions.map((m) => (
          <SessionChip
            key={m.id}
            meta={m}
            hidden={true}
            selected={selectedId === m.id}
            live={liveById.get(m.id)}
            onSelect={onSelect}
            onSetHidden={onSetHidden}
            onContextMenu={openChipMenu}
          />
        ))}
      {scriptChips.length > 0 && <span className="chip-bar-divider" />}
      {scriptChips.map((chip) => {
        const key = scriptTabKey(chip.repoKey, chip.scriptName);
        return (
          <ScriptChip
            key={key}
            chip={chip}
            showRepo={showRepoOnScript}
            selected={selectedScriptKey === key}
            onClick={onScriptChipClick}
            onDismiss={onScriptChipDismiss}
          />
        );
      })}
      <div className="new-session-wrap" ref={wrapRef}>
        <button
          type="button"
          className="session-chip new"
          onClick={onNewClick}
          disabled={busy || repos.length === 0}
          title={
            repos.length === 0
              ? "No repos in this workspace"
              : repos.length === 1
                ? `Start a new Claude session in ${repos[0].repo_key}`
                : "Start a new Claude session"
          }
        >
          {busy ? <Spinner /> : "+"} New
          {repos.length > 1 && <span className="caret">▾</span>}
        </button>
        {menuOpen && repos.length > 1 && (
          <div className="new-session-menu" role="menu">
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setMenuOpen(false);
                onStartInRepo(null);
              }}
            >
              New at workspace <code>root</code>
            </button>
            {repos.map((r) => (
              <button
                key={r.repo_key}
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenuOpen(false);
                  onStartInRepo(r.repo_key);
                }}
              >
                New in <code>{r.repo_key}</code>
              </button>
            ))}
          </div>
        )}
      </div>
      {hiddenSessions.length > 0 && (
        <button
          type="button"
          className="hidden-toggle"
          onClick={onToggleShowHidden}
          title={showHidden ? "Hide hidden chats" : "Show hidden chats"}
        >
          {showHidden
            ? `Hide ${hiddenSessions.length} hidden`
            : `Show ${hiddenSessions.length} hidden`}
        </button>
      )}
      {chipMenu &&
        (() => {
          const live = liveById.get(chipMenu.sessionId);
          const needsTurn =
            !!live?.running &&
            !live.turn_acknowledged &&
            (live.runtime_state === "idle" ||
              live.runtime_state === "waiting_input");
          const isHidden = hiddenSessions.some(
            (m) => m.id === chipMenu.sessionId,
          );
          const meta = [...visibleSessions, ...hiddenSessions].find(
            (m) => m.id === chipMenu.sessionId,
          );
          const currentBinary =
            meta?.claude_binary ?? workspaceBinary ?? "claude";
          return (
            <SessionChipMenu
              x={chipMenu.x}
              y={chipMenu.y}
              needsTurn={needsTurn}
              hidden={isHidden}
              currentBinary={currentBinary}
              onClearTurn={() => onClearTurn(chipMenu.sessionId)}
              onSetHidden={() => onSetHidden(chipMenu.sessionId, !isHidden)}
              onSwitchBinary={(binary) =>
                onSwitchBinary(chipMenu.sessionId, binary)
              }
              onClose={() => setChipMenu(null)}
            />
          );
        })()}
    </div>
  );
}

function SessionChipMenu({
  x,
  y,
  needsTurn,
  hidden,
  currentBinary,
  onClearTurn,
  onSetHidden,
  onSwitchBinary,
  onClose,
}: {
  x: number;
  y: number;
  needsTurn: boolean;
  hidden: boolean;
  currentBinary: string;
  onClearTurn: () => void;
  onSetHidden: () => void;
  onSwitchBinary: (binary: string) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const handle = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", handle);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", handle);
      document.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  // Keep the menu inside the viewport.
  const ESTIMATED_W = 200;
  const ESTIMATED_H = 240;
  const left = Math.min(x, window.innerWidth - ESTIMATED_W - 4);
  const top = Math.min(y, window.innerHeight - ESTIMATED_H - 4);

  const wrap = (fn: () => void) => () => {
    fn();
    onClose();
  };

  return (
    <div ref={ref} className="context-menu" style={{ left, top }} role="menu">
      {needsTurn && (
        <button
          type="button"
          role="menuitem"
          onClick={wrap(onClearTurn)}
        >
          Clear notification
        </button>
      )}
      <button type="button" role="menuitem" onClick={wrap(onSetHidden)}>
        {hidden ? "Show this chat" : "Hide this chat"}
      </button>
      <div className="context-menu-sep" />
      <div className="context-menu-label">Run with</div>
      {CLAUDE_BINARIES.map((b) => {
        const isCurrent = b === currentBinary;
        return (
          <button
            key={b}
            type="button"
            role="menuitem"
            disabled={isCurrent}
            onClick={wrap(() => onSwitchBinary(b))}
          >
            {isCurrent ? `${b} ✓` : b}
          </button>
        );
      })}
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
function AttachedPrChips({
  link,
  onDetach,
}: {
  link: RepoLink;
  onDetach: (repoKey: string, prNumber: number) => void;
}) {
  return (
    <>
      {link.attached_prs.map((attached) =>
        attached.status ? (
          <GithubChip
            key={attached.number}
            status={attached.status}
            onDetach={() => onDetach(link.repo_key, attached.number)}
          />
        ) : (
          // Attaching fetches the PR up front, so this only shows up if the PR
          // later became unreachable (deleted, or GitHub is down).
          <span
            key={attached.number}
            className="gh-chip gh-chip-missing"
            title={`PR #${attached.number} in ${link.repo_key} couldn't be fetched`}
          >
            <span className="gh-pr">#{attached.number}</span>
            <span className="gh-draft-badge">no data</span>
            <PrDetachButton
              prNumber={attached.number}
              onDetach={() => onDetach(link.repo_key, attached.number)}
            />
          </span>
        ),
      )}
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
      await invoke<GithubPrStatus>("attach_pr", {
        args: {
          workspace_id: workspace.id,
          repo_key: repoKey,
          reference: reference.trim(),
        },
      });
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

function AddRepoDialog({
  workspace,
  availableRepos,
  onClose,
  onSuccess,
}: {
  workspace: Workspace;
  availableRepos: Repo[];
  onClose: () => void;
  onSuccess: () => void;
}) {
  const [picked, setPicked] = useState<string | null>(null);
  // Setting `tempId` flips the dialog from picker → job-log mode and triggers
  // useBackendJob (which only fires when `descriptor` is non-null).
  const [tempId, setTempId] = useState<string | null>(null);

  const descriptor = useMemo<JobDescriptor | null>(() => {
    if (!tempId || !picked) return null;
    return {
      key: tempId,
      command: "add_repo_to_workspace",
      args: { args: { workspace_id: workspace.id, repo_key: picked } },
    };
  }, [tempId, picked, workspace.id]);

  const { events, state } = useBackendJob(descriptor, {
    onSuccess: () => onSuccess(),
  });

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!picked) return;
    setTempId(crypto.randomUUID());
  };

  const isRunning = tempId !== null && state === "running";

  return (
    <div
      className="modal-backdrop"
      onClick={isRunning ? undefined : onClose}
    >
      <div
        className={`modal${tempId ? " add-repo-modal-running" : ""}`}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        {!tempId ? (
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
        ) : (
          <JobLogPane
            title={`Adding ${picked} to ${workspace.branch}`}
            events={events}
            state={state}
            onDismiss={onClose}
          />
        )}
      </div>
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

/** Dialog emits everything *except* the workspace id — App mints that and
 *  merges it in before invoking. */
type CreateWorkspaceFormArgs = Omit<CreateWorkspaceArgs, "workspace_id">;

function CreateWorkspaceDialog({
  repos,
  onClose,
  onSubmit,
}: {
  repos: Repo[];
  onClose: () => void;
  onSubmit: (args: CreateWorkspaceFormArgs) => void;
}) {
  const [branch, setBranch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(() =>
    loadLastRepoSelection(repos),
  );
  const [claudeBinary, setClaudeBinary] = useState("claude");

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
    } catch {
      // non-fatal: preference just won't persist
    }
    onSubmit({
      branch: branch.trim(),
      repo_selections: repoSelections,
      claude_binary: claudeBinary === "claude" ? null : claudeBinary,
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
        <label>
          Claude binary
          <select
            value={claudeBinary}
            onChange={(e) => setClaudeBinary(e.target.value)}
          >
            {CLAUDE_BINARIES.map((b) => (
              <option key={b} value={b}>
                {b}
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
