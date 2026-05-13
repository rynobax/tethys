import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  horizontalListSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import type {
  CreateWorkspaceArgs,
  Discrepancies,
  GithubStatusChangedEvent,
  MemorySnapshot,
  RegistryStatus,
  Repo,
  SessionInfo,
  SessionKind,
  SessionRuntimeState,
  Theme,
  TurnChangedEvent,
  Workspace,
  WorkspaceId,
} from "./types";
import { DevServerActions, SidebarMemoryPressure } from "./DevServerControls";
import { GithubAuthFooter } from "./GithubAuthFooter";
import { GithubChip } from "./GithubChip";
import { JobLogPane } from "./JobLogPane";
import { SessionTerminal } from "./SessionTerminal";
import { Sidebar } from "./Sidebar";
import { SystemStatus } from "./SystemStatus";
import { applyTheme, ThemeContext } from "./theme";
import { useBackendJob, type JobDescriptor } from "./useBackendJob";
import { useTauriEvent } from "./useTauriEvent";
import { isReadyToDelete } from "./workspaceDerived";
import "./App.css";

function App() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [registry, setRegistry] = useState<RegistryStatus | null>(null);
  const [discrepancies, setDiscrepancies] = useState<Discrepancies | null>(null);
  const [selectedId, setSelectedId] = useState<WorkspaceId | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /**
   * Latest memory snapshot. Subscribed once at the app level and passed
   * down to Sidebar (footer pressure + per-row RAM chips) and to the
   * dev-server action buttons (for the start gate). The 5s poll lands
   * here; per-workspace bits flow out via `memory.per_workspace`.
   */
  const [memory, setMemory] = useState<MemorySnapshot | null>(null);
  useTauriEvent<MemorySnapshot>("devserver:memory_updated", (e) => {
    setMemory(e.payload);
  });
  // Seed the snapshot once so the UI isn't blank before the first poll.
  useEffect(() => {
    invoke<MemorySnapshot>("get_memory_snapshot")
      .then(setMemory)
      .catch(() => {});
  }, []);
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
    const { workspace_id, repo_key, status } = event.payload;
    setWorkspaces((prev) =>
      prev.map((w) => {
        if (w.id !== workspace_id) return w;
        return {
          ...w,
          repo_links: w.repo_links.map((r) =>
            r.repo_key === repo_key ? { ...r, github: status } : r,
          ),
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
      // Pre-load sessions for every workspace so switching in doesn't
      // render a stale/empty sessions list.
      await Promise.all(list.map((w) => refreshSessionsFor(w.id)));
    } catch (e) {
      setError(String(e));
    }
  }, [refreshSessionsFor]);

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
  });
  navRef.current = {
    list: navigableWorkspaces,
    selectedId,
    needsTurn: workspaceNeedsTurn,
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
      // Cmd+Alt(+Shift) + J/K. `e.code` ignores Option's character
      // remapping on macOS (Alt+J → ˝), so the binding survives layout.
      if (!e.metaKey || !e.altKey || e.ctrlKey) return;
      if (e.code !== "KeyJ" && e.code !== "KeyK") return;
      const direction: 1 | -1 = e.code === "KeyK" ? 1 : -1;
      e.preventDefault();
      e.stopPropagation();
      step(direction, e.shiftKey);
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
          memory={memory}
        />
        <div className="sidebar-footer">
          <SidebarMemoryPressure memory={memory} />
          <SystemStatus allWorkspaces={workspaces} />
          <GithubAuthFooter />
        </div>
      </aside>

      <main className="detail">
        {error && <div className="error-banner">{error}</div>}
        {registry && !registryOk && (
          <RegistryNotice registry={registry} onChanged={refresh} />
        )}
        {discrepancies &&
          (discrepancies.orphaned_dirs.length > 0 ||
            discrepancies.missing_worktrees.length > 0) && (
            <DiscrepancyNotice
              discrepancies={discrepancies}
              onChanged={refresh}
            />
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
            onSuccess={handleCreateSuccess}
            onDismiss={() => handleCreationDismiss(id)}
          />
        ))}
        {!selectedRun && selected && selected.status.kind === "ready" && (
          <WorkspaceDetail
            workspace={selected}
            sessions={sessionsByWorkspace.get(selected.id) ?? []}
            availableRepos={
              registry?.kind === "ok"
                ? registry.registry.repos.filter(
                    (r) =>
                      !selected.repo_links.some((l) => l.repo_key === r.key),
                  )
                : []
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
  onSuccess,
  onDismiss,
}: {
  workspaceId: WorkspaceId;
  args: CreateWorkspaceArgs;
  isShown: boolean;
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
    <JobLogPane
      title={`Creating ${args.branch}`}
      events={events}
      state={state}
      onDismiss={onDismiss}
    />
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

function DiscrepancyNotice({
  discrepancies,
  onChanged,
}: {
  discrepancies: Discrepancies;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<Set<string>>(() => new Set());

  const withBusy = async (key: string, fn: () => Promise<void>) => {
    setBusy((prev) => new Set(prev).add(key));
    try {
      await fn();
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }
  };

  const removeOrphan = (path: string) =>
    withBusy(`orphan:${path}`, async () => {
      await invoke("remove_orphan_dir", { path });
      onChanged();
    });

  const forgetWorkspace = (id: string) =>
    withBusy(`forget:${id}`, async () => {
      await invoke("forget_workspace", { id });
      onChanged();
    });

  // Collapse missing-worktree rows by workspace_id — multiple repos per
  // workspace would otherwise show redundant Forget buttons.
  const missingByWorkspace = new Map<
    string,
    { branch: string; repos: string[] }
  >();
  for (const m of discrepancies.missing_worktrees) {
    const entry = missingByWorkspace.get(m.workspace_id) ?? {
      branch: m.branch,
      repos: [],
    };
    entry.repos.push(m.repo_key);
    missingByWorkspace.set(m.workspace_id, entry);
  }

  return (
    <div className="discrepancy-notice">
      <h2>State / disk mismatch</h2>
      <p>
        Tethys found things that don't line up between <code>state.json</code>{" "}
        and your <code>worktree_root</code>. Usually the result of a crash or
        manual filesystem surgery.
      </p>

      {discrepancies.orphaned_dirs.length > 0 && (
        <>
          <h3>Orphaned worktrees</h3>
          <p className="muted">
            Directories with no matching workspace in state. Safe to remove.
          </p>
          <ul className="discrepancy-list">
            {discrepancies.orphaned_dirs.map((o) => {
              const working = busy.has(`orphan:${o.path}`);
              return (
                <li key={o.path}>
                  <code className="discrepancy-path">{o.path}</code>
                  <button
                    type="button"
                    className="danger"
                    onClick={() => removeOrphan(o.path)}
                    disabled={working}
                  >
                    {working ? (
                      <>
                        <Spinner /> Removing…
                      </>
                    ) : (
                      "Remove"
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        </>
      )}

      {missingByWorkspace.size > 0 && (
        <>
          <h3>Missing worktrees</h3>
          <p className="muted">
            Workspaces in state whose worktree directories have vanished. Forget
            drops the workspace from state; it does not touch disk.
          </p>
          <ul className="discrepancy-list">
            {Array.from(missingByWorkspace.entries()).map(
              ([id, { branch, repos }]) => {
                const working = busy.has(`forget:${id}`);
                return (
                  <li key={id}>
                    <div className="discrepancy-meta">
                      <code>{branch}</code>{" "}
                      <span className="muted">
                        · missing: {repos.join(", ")}
                      </span>
                    </div>
                    <button
                      type="button"
                      className="danger"
                      onClick={() => forgetWorkspace(id)}
                      disabled={working}
                    >
                      {working ? (
                        <>
                          <Spinner /> Forgetting…
                        </>
                      ) : (
                        "Forget"
                      )}
                    </button>
                  </li>
                );
              },
            )}
          </ul>
        </>
      )}
    </div>
  );
}

function WorkspaceDetail({
  workspace,
  sessions,
  availableRepos,
  onRequestDelete,
  onRequestArchive,
  onRepoAdded,
}: {
  workspace: Workspace;
  sessions: SessionInfo[];
  availableRepos: Repo[];
  onRequestDelete: () => void;
  onRequestArchive: () => void;
  onRepoAdded: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [showInfo, setShowInfo] = useState(false);
  const [addingRepo, setAddingRepo] = useState(false);
  // Per-workspace selection. Derived on render (no effect), so switching
  // back to a workspace paints the remembered pick immediately.
  const [selectedByWorkspace, setSelectedByWorkspace] = useState<
    Map<string, string>
  >(new Map());
  const selectedSessionId = selectedByWorkspace.get(workspace.id) ?? null;
  const selectSession = (id: string | null) => {
    setSelectedByWorkspace((prev) => {
      const next = new Map(prev);
      if (id) next.set(workspace.id, id);
      else next.delete(workspace.id);
      return next;
    });
  };
  const [error, setError] = useState<string | null>(null);
  // Meta ids we've already auto-resumed this app-run — guards against
  // retry loops if spawn fails, while still allowing a manual Resume
  // click to try again.
  const autoResumedRef = useRef<Set<string>>(new Set());

  const liveById = new Map(sessions.map((s) => [s.id, s]));
  // Ordering:
  //   - If `session_order` is set (user has manually dragged), use that
  //     verbatim. Any session id present in `workspace.sessions` but not
  //     in the pin gets appended so newly-spawned sessions appear.
  //   - Otherwise, default to "Claude newest-first, then FE/BE Build
  //     chips at the end" so dev-server tabs don't push Claude chats
  //     around when servers start.
  const isDev = (m: typeof workspace.sessions[number]) =>
    m.kind === "frontend_build" || m.kind === "backend_build";
  const ordered = (() => {
    const reversed = [...workspace.sessions].reverse();
    const pin = workspace.session_order;
    if (!pin || pin.length === 0) {
      return [
        ...reversed.filter((m) => !isDev(m)),
        ...reversed.filter(isDev),
      ];
    }
    const byId = new Map(workspace.sessions.map((s) => [s.id, s]));
    const seen = new Set<string>();
    const result: typeof workspace.sessions = [];
    for (const id of pin) {
      const s = byId.get(id);
      if (s && !seen.has(id)) {
        result.push(s);
        seen.add(id);
      }
    }
    // Append any unseen sessions — keep the "Claude before dev" tiebreak
    // for the new ones so they don't all clump at the end mid-list.
    const tail = reversed.filter((s) => !seen.has(s.id));
    return [
      ...result,
      ...tail.filter((m) => !isDev(m)),
      ...tail.filter(isDev),
    ];
  })();
  const visibleOrdered = ordered.filter((m) => !m.hidden);
  const hiddenOrdered = ordered.filter((m) => m.hidden);
  const [showHidden, setShowHidden] = useState(false);
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(
    null,
  );

  const persistOrder = useCallback(
    async (newVisibleOrder: string[]) => {
      // Merge: persist the visible order followed by the current hidden
      // order. The backend dedupes if anything overlaps.
      const hiddenIds = hiddenOrdered.map((m) => m.id);
      try {
        await invoke("reorder_sessions", {
          args: {
            workspace_id: workspace.id,
            session_ids: [...newVisibleOrder, ...hiddenIds],
          },
        });
      } catch (e) {
        console.error("reorder_sessions:", e);
      }
    },
    [workspace.id, hiddenOrdered],
  );

  const renameSession = useCallback(
    async (sessionId: string, displayName: string | null) => {
      try {
        await invoke("rename_session", {
          args: {
            workspace_id: workspace.id,
            session_id: sessionId,
            display_name: displayName,
          },
        });
      } catch (e) {
        console.error("rename_session:", e);
      }
    },
    [workspace.id],
  );

  // Effective selection: prefer the user's explicit pick when it's still
  // visible, else fall through to first live, else newest. When hiding
  // the selected chip, this naturally jumps to the next visible one.
  const candidates = showHidden ? ordered : visibleOrdered;
  const effectiveSelected = (() => {
    if (selectedSessionId && candidates.some((m) => m.id === selectedSessionId))
      return selectedSessionId;
    const firstLive = candidates.find((m) => liveById.has(m.id));
    return firstLive?.id ?? candidates[0]?.id ?? null;
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

  // Auto-resume the selected session when it's dormant but has a
  // claude_session_id. `autoResumedRef` prevents a retry loop if the
  // spawn fails — the user can still click Resume manually below.
  useEffect(() => {
    if (!selected || selectedLive) return;
    if (!selected.claude_session_id) return;
    if (autoResumedRef.current.has(selected.id)) return;
    autoResumedRef.current.add(selected.id);
    void resumeMeta(selected.id, selected.repo_key);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.id, selectedLive?.id, selected?.claude_session_id]);

  return (
    <div className="workspace-detail">
      <header>
        <h2>
          <code>{workspace.branch}</code>
          {workspace.repo_links.map(
            (r) => r.github && <GithubChip key={r.repo_key} status={r.github} />,
          )}
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
          <DevServerActions workspace={workspace} onChanged={onRepoAdded} />
          <button
            type="button"
            className="danger"
            onClick={onRequestDelete}
            disabled={busy}
          >
            Delete
          </button>
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
          onReorder={persistOrder}
          renamingSessionId={renamingSessionId}
          onStartRename={(id) => setRenamingSessionId(id)}
          onCommitRename={async (id, name) => {
            await renameSession(id, name);
            setRenamingSessionId(null);
          }}
          onCancelRename={() => setRenamingSessionId(null)}
          busy={busy}
        />
        {error && <div className="error-banner">{error}</div>}
        {selected ? (
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
              <SessionTerminal
                sessionId={selectedLive.id}
                readOnly={
                  selected.kind === "frontend_build" ||
                  selected.kind === "backend_build"
                }
              />
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
  /** User-set chip label override. Ignored for dev-server kinds. */
  display_name?: string | null;
  /** Defaults to "claude" on older state.json entries; non-claude kinds
   *  (frontend_build / backend_build) render with a fixed "Local Build"
   *  label, skip the hide/rename/drag affordances. */
  kind?: SessionKind;
};

function SessionChip({
  meta,
  hidden,
  selected,
  live,
  isRenaming,
  draggable,
  onSelect,
  onSetHidden,
  onStartRename,
  onCommitRename,
  onCancelRename,
}: {
  meta: ChipMeta;
  hidden: boolean;
  selected: boolean;
  live: SessionInfo | undefined;
  /** True when this chip is the one currently being renamed. Shows
   *  an inline <input> in place of the default label. */
  isRenaming: boolean;
  /** True when the chip should be a dnd-kit sortable item. Hidden
   *  chips (in the "show hidden" drawer) aren't draggable in V1 to
   *  keep the reorder UX scoped to what the user normally sees. */
  draggable: boolean;
  onSelect: (id: string) => void;
  onSetHidden: (id: string, hidden: boolean) => void;
  onStartRename: (id: string) => void;
  onCommitRename: (id: string, name: string | null) => void;
  onCancelRename: () => void;
}) {
  const isDevServer =
    meta.kind === "frontend_build" || meta.kind === "backend_build";
  // Dev-server chips don't participate in drag-to-reorder or rename —
  // they're pinned to the end by default and carry a fixed label.
  // `useSortable` is still called (hook rule) but disabled.
  const sortable = useSortable({
    id: meta.id,
    disabled: !draggable || isDevServer,
  });
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = sortable;
  const defaultLabel = isDevServer ? "Local Build" : meta.id.slice(0, 8);
  const customLabel = isDevServer ? null : meta.display_name?.trim();
  const labelText = customLabel || defaultLabel;
  const needsTurn =
    !isDevServer &&
    live?.running &&
    (live.runtime_state === "idle" || live.runtime_state === "waiting_input");
  const chipClass = [
    "session-chip",
    selected ? "active" : "",
    hidden ? "hidden" : "",
    isDragging ? "is-dragging" : "",
    customLabel ? "renamed" : "",
    isDevServer ? "dev-server" : "",
  ]
    .filter(Boolean)
    .join(" ");
  const style: React.CSSProperties = draggable
    ? {
        transform: CSS.Transform.toString(transform),
        transition,
        // With DragOverlay handling the visual preview, the in-place
        // chip is just a placeholder. Fade it more so the slot reads
        // as "reserved" rather than "two duplicates of this chip".
        opacity: isDragging ? 0.25 : undefined,
      }
    : {};
  return (
    <div
      ref={draggable ? setNodeRef : undefined}
      style={style}
      {...(draggable ? attributes : {})}
      {...(draggable ? listeners : {})}
      role="button"
      tabIndex={0}
      className={chipClass}
      onClick={() => {
        if (isRenaming) return;
        onSelect(meta.id);
      }}
      onDoubleClick={(e) => {
        if (isDevServer) return;
        e.preventDefault();
        e.stopPropagation();
        onStartRename(meta.id);
      }}
      onKeyDown={(e) => {
        if (isRenaming) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect(meta.id);
        }
      }}
    >
      <span className={`chip-repo${meta.repo_key === null ? " root" : ""}`}>
        {meta.repo_key ?? "root"}
      </span>
      {isRenaming ? (
        <ChipRenameInput
          initial={customLabel ?? ""}
          placeholder={defaultLabel}
          onCommit={(value) =>
            onCommitRename(meta.id, value.trim() || null)
          }
          onCancel={onCancelRename}
        />
      ) : (
        <code title={customLabel ? `Default: ${defaultLabel}` : undefined}>
          {labelText}
        </code>
      )}
      {needsTurn && <span className="turn-dot" />}
      {live && !live.running && <span className="chip-state">exited</span>}
      {!live && !isDevServer && <span className="chip-state">dormant</span>}
      {!isDevServer && (
        <button
          type="button"
          className="chip-x"
          title={hidden ? "Show this chat" : "Hide this chat"}
          aria-label={hidden ? "Show this chat" : "Hide this chat"}
          onClick={(e) => {
            e.stopPropagation();
            onSetHidden(meta.id, !hidden);
          }}
          // Prevent the dnd-kit drag listener from picking up clicks on
          // the close button (otherwise dragging the X moves the chip).
          onPointerDown={(e) => e.stopPropagation()}
        >
          {hidden ? "↺" : "×"}
        </button>
      )}
    </div>
  );
}

/** Tiny controlled-input used during chip rename. Mounts focused, saves
 *  on Enter / blur, cancels on Escape. */
function ChipRenameInput({
  initial,
  placeholder,
  onCommit,
  onCancel,
}: {
  initial: string;
  placeholder: string;
  onCommit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  return (
    <input
      ref={ref}
      className="chip-rename-input"
      value={value}
      placeholder={placeholder}
      onChange={(e) => setValue(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          onCommit(value);
        } else if (e.key === "Escape") {
          e.preventDefault();
          onCancel();
        }
        // Stop the chip's outer keydown handler from running (it would
        // select the chip on Enter/Space).
        e.stopPropagation();
      }}
      onBlur={() => onCommit(value)}
    />
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
  onReorder,
  renamingSessionId,
  onStartRename,
  onCommitRename,
  onCancelRename,
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
  /** Persist a new chip order. Receives the visible chips' new order;
   *  the caller is responsible for appending hidden ids if any. */
  onReorder: (newVisibleOrder: string[]) => void;
  renamingSessionId: string | null;
  onStartRename: (id: string) => void;
  onCommitRename: (id: string, name: string | null) => void;
  onCancelRename: () => void;
  busy: boolean;
}) {
  // dnd-kit horizontal sortable. 5px activation distance is the same
  // value the Sidebar uses to avoid swallowing chip clicks.
  const dndSensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  // Optimistic local order. On drop we apply the new order *immediately*
  // so dnd-kit's drop animation lands on the correct geometry — without
  // this, the chip strip keeps rendering the old order until the
  // backend's `workspace:changed` event lands, and the visible
  // collapse-then-re-expand looks like a flicker. Cleared once the
  // props catch up (so a server-side reconcile can still re-order).
  const [optimisticOrder, setOptimisticOrder] = useState<string[] | null>(
    null,
  );
  const visibleIds = visibleSessions.map((m) => m.id);
  useEffect(() => {
    if (!optimisticOrder) return;
    if (
      visibleIds.length === optimisticOrder.length &&
      visibleIds.every((id, i) => id === optimisticOrder[i])
    ) {
      setOptimisticOrder(null);
    }
    // visibleIds is derived; deep-compare via the joined string.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visibleIds.join(""), optimisticOrder]);

  const effectiveVisible = (() => {
    if (!optimisticOrder) return visibleSessions;
    const byId = new Map(visibleSessions.map((s) => [s.id, s]));
    const result: ChipMeta[] = [];
    const seen = new Set<string>();
    for (const id of optimisticOrder) {
      const s = byId.get(id);
      if (s && !seen.has(id)) {
        result.push(s);
        seen.add(id);
      }
    }
    for (const s of visibleSessions) {
      if (!seen.has(s.id)) result.push(s);
    }
    return result;
  })();

  // The id of the chip currently being dragged. Rendered into a
  // DragOverlay below so the visible "preview" stays its natural size
  // — without the overlay, dnd-kit shifts neighbors by the dragged
  // chip's width, which on a variable-width row makes the moving chip
  // appear to stretch/shrink to match each chip it crosses.
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const draggingItem = draggingId
    ? effectiveVisible.find((m) => m.id === draggingId) ?? null
    : null;

  const handleDragStart = (event: DragStartEvent) => {
    setDraggingId(event.active.id as string);
  };

  const handleDragEnd = (event: DragEndEvent) => {
    setDraggingId(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const ids = effectiveVisible.map((m) => m.id);
    const from = ids.indexOf(active.id as string);
    const to = ids.indexOf(over.id as string);
    if (from < 0 || to < 0) return;
    const next = arrayMove(ids, from, to);
    setOptimisticOrder(next);
    onReorder(next);
  };
  const [menuOpen, setMenuOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

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
      <DndContext
        sensors={dndSensors}
        collisionDetection={closestCenter}
        onDragStart={handleDragStart}
        onDragEnd={handleDragEnd}
        onDragCancel={() => setDraggingId(null)}
      >
        <SortableContext
          items={effectiveVisible.map((m) => m.id)}
          strategy={horizontalListSortingStrategy}
        >
          {effectiveVisible.map((m) => (
            <SessionChip
              key={m.id}
              meta={m}
              hidden={false}
              selected={selectedId === m.id}
              live={liveById.get(m.id)}
              isRenaming={renamingSessionId === m.id}
              draggable
              onSelect={onSelect}
              onSetHidden={onSetHidden}
              onStartRename={onStartRename}
              onCommitRename={onCommitRename}
              onCancelRename={onCancelRename}
            />
          ))}
        </SortableContext>
        <DragOverlay>
          {draggingItem ? (
            <SessionChip
              meta={draggingItem}
              hidden={false}
              selected={selectedId === draggingItem.id}
              live={liveById.get(draggingItem.id)}
              isRenaming={false}
              draggable={false}
              onSelect={() => {}}
              onSetHidden={() => {}}
              onStartRename={() => {}}
              onCommitRename={() => {}}
              onCancelRename={() => {}}
            />
          ) : null}
        </DragOverlay>
      </DndContext>
      {showHidden &&
        hiddenSessions.map((m) => (
          <SessionChip
            key={m.id}
            meta={m}
            hidden={true}
            selected={selectedId === m.id}
            live={liveById.get(m.id)}
            isRenaming={renamingSessionId === m.id}
            draggable={false}
            onSelect={onSelect}
            onSetHidden={onSetHidden}
            onStartRename={onStartRename}
            onCommitRename={onCommitRename}
            onCancelRename={onCancelRename}
          />
        ))}
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
  const [useHipaa, setUseHipaa] = useState(false);

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
      claude_binary: useHipaa ? "claude-hipaa" : null,
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
        <label className="repo-row">
          <input
            type="checkbox"
            checked={useHipaa}
            onChange={(e) => setUseHipaa(e.target.checked)}
          />
          <span className="repo-display">
            Use <code>claude-hipaa</code> for sessions in this workspace
          </span>
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
