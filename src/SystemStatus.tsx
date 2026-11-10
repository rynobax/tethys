import { useCallback, useEffect, useState } from "react";
import * as api from "./ipc/commands";

import type {
  Discrepancies,
  RegistryStatus,
  SystemErrorEntry,
  Workspace,
  WorkspaceId,
} from "./types";
import { useAppEvent } from "./ipc/events";

type Props = {
  /** Including soft-deleted ones. */
  allWorkspaces: Workspace[];
  registry: RegistryStatus | null;
  discrepancies: Discrepancies | null;
  onDiscrepancyChange: () => void;
};

const HOUR_MS = 60 * 60 * 1000;

export function SystemStatus({
  allWorkspaces,
  registry,
  discrepancies,
  onDiscrepancyChange,
}: Props) {
  const [errors, setErrors] = useState<SystemErrorEntry[]>([]);
  const [open, setOpen] = useState(false);

  const refreshErrors = useCallback(async () => {
    try {
      const list = await api.listSystemErrors();
      setErrors(list);
    } catch (e) {
      console.error("list_system_errors:", e);
    }
  }, []);

  useEffect(() => {
    refreshErrors();
  }, [refreshErrors]);

  useAppEvent("system_status:changed", () => refreshErrors());

  const pendingDeletes = allWorkspaces.filter((w) => w.deleted_at !== null);
  const hasErrors = errors.length > 0;
  // Pending deletions are routine, so they don't warn.
  const hasDiscrepancies =
    (discrepancies?.orphaned_dirs.length ?? 0) > 0 ||
    (discrepancies?.missing_worktrees.length ?? 0) > 0;

  return (
    <>
      <button
        type="button"
        className={`system-status-button${hasErrors ? " has-errors" : ""}`}
        onClick={() => setOpen(true)}
        title="System status"
      >
        <span
          className={`status-dot ${
            hasErrors ? "red" : hasDiscrepancies ? "yellow" : "green"
          }`}
        />
        Status
      </button>
      {open && (
        <SystemStatusModal
          errors={errors}
          pendingDeletes={pendingDeletes}
          discrepancies={discrepancies}
          registry={registry}
          onClose={() => setOpen(false)}
          onRefreshErrors={refreshErrors}
          onDiscrepancyChange={onDiscrepancyChange}
        />
      )}
    </>
  );
}

function SystemStatusModal({
  errors,
  pendingDeletes,
  discrepancies,
  registry,
  onClose,
  onRefreshErrors,
  onDiscrepancyChange,
}: {
  errors: SystemErrorEntry[];
  pendingDeletes: Workspace[];
  discrepancies: Discrepancies | null;
  registry: RegistryStatus | null;
  onClose: () => void;
  onRefreshErrors: () => void;
  onDiscrepancyChange: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal system-status-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <h3>System status</h3>

        <StatusSections
          errors={errors}
          pendingDeletes={pendingDeletes}
          discrepancies={discrepancies}
          registry={registry}
          onRefresh={onRefreshErrors}
          onDiscrepancyChange={onDiscrepancyChange}
        />

        <div className="modal-actions">
          <button type="button" onClick={onClose} autoFocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

function StatusSections({
  errors,
  pendingDeletes,
  discrepancies,
  registry,
  onRefresh,
  onDiscrepancyChange,
}: {
  errors: SystemErrorEntry[];
  pendingDeletes: Workspace[];
  discrepancies: Discrepancies | null;
  registry: RegistryStatus | null;
  onRefresh: () => void;
  onDiscrepancyChange: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [cloneDir, setCloneDir] = useState<string | null>(null);

  useEffect(() => {
    api
      .cloneDirPath()
      .then(setCloneDir, (e) => console.error("clone_dir_path:", e));
  }, []);

  const cancelDelete = async (id: WorkspaceId) => {
    setBusy(`cancel:${id}`);
    try {
      await api.cancelDeleteWorkspace(id);
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
    }
  };

  const dismissError = async (id: string) => {
    setBusy(`err:${id}`);
    try {
      await api.dismissSystemError(id);
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
    }
  };

  const runCleanupNow = async () => {
    setBusy("cleanup");
    try {
      await api.runPurgeNow();
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
      onRefresh();
    }
  };

  const openLocation = async (location: api.ConfigLocation) => {
    try {
      await api.openConfigLocation(location);
    } catch (e) {
      alert(String(e));
    }
  };

  return (
    <>
      <DiskMismatchSection
        discrepancies={discrepancies}
        onChanged={onDiscrepancyChange}
      />

      <section>
        <h4>Configuration</h4>
        {registry ? (
          <ul className="status-list">
            <li>
              <PathRow
                label="repos.toml"
                path={registry.path}
                onOpen={() => openLocation("repos_config")}
              />
            </li>
            {registry.kind === "ok" && (
              <li>
                <PathRow
                  label="worktree_root"
                  path={registry.registry.worktree_root}
                  onOpen={() => openLocation("worktree_root")}
                />
              </li>
            )}
            {cloneDir && (
              <li>
                <PathRow
                  label="base clones"
                  path={cloneDir}
                  onOpen={() => openLocation("clone_dir")}
                />
              </li>
            )}
          </ul>
        ) : (
          <p className="muted">Registry status not loaded yet.</p>
        )}
      </section>

      <section>
        <div className="section-header">
          <h4>Pending deletions</h4>
          <button type="button" onClick={runCleanupNow} disabled={busy !== null}>
            Run cleanup now
          </button>
        </div>
        {pendingDeletes.length === 0 ? (
          <p className="muted">Nothing waiting to be cleaned up.</p>
        ) : (
          <ul className="status-list">
            {pendingDeletes.map((w) => {
              const deletedAt = w.deleted_at
                ? new Date(w.deleted_at).getTime()
                : 0;
              const ageMs = Date.now() - deletedAt;
              const eligible = ageMs >= HOUR_MS;
              const label = eligible
                ? "Ready to purge on next tick"
                : `Will purge after ${formatRemaining(HOUR_MS - ageMs)}`;
              return (
                <li key={w.id}>
                  <div className="status-row">
                    <code>{w.branch}</code>
                    <span className="muted">{label}</span>
                  </div>
                  <button
                    type="button"
                    onClick={() => cancelDelete(w.id)}
                    disabled={busy === `cancel:${w.id}`}
                  >
                    Cancel deletion
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section>
        <h4>Errors</h4>
        {errors.length === 0 ? (
          <p className="muted">No errors recorded.</p>
        ) : (
          <ul className="status-list">
            {errors
              .slice()
              .reverse()
              .map((err) => (
                <li key={err.id} className="error-entry">
                  <div className="status-row">
                    <span className="error-when">
                      {new Date(err.at).toLocaleString()}
                    </span>
                    {err.workspace_branch && <code>{err.workspace_branch}</code>}
                    <span className="error-kind">{err.kind}</span>
                  </div>
                  <pre className="error-message">{err.message}</pre>
                  <button
                    type="button"
                    onClick={() => dismissError(err.id)}
                    disabled={busy === `err:${err.id}`}
                  >
                    Dismiss
                  </button>
                </li>
              ))}
          </ul>
        )}
      </section>
    </>
  );
}

/** The path stays plain text so it can be copied. */
function PathRow({
  label,
  path,
  onOpen,
}: {
  label: string;
  path: string;
  onOpen: () => void;
}) {
  return (
    <div className="status-row">
      <button
        type="button"
        className="path-label"
        title={`Open ${label} in the editor`}
        onClick={onOpen}
      >
        {label}
      </button>
      <code>{path}</code>
    </div>
  );
}

function DiskMismatchSection({
  discrepancies,
  onChanged,
}: {
  discrepancies: Discrepancies | null;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<Set<string>>(() => new Set());

  const orphans = discrepancies?.orphaned_dirs ?? [];
  const missing = discrepancies?.missing_worktrees ?? [];
  if (orphans.length === 0 && missing.length === 0) return null;

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
      await api.removeOrphanDir(path);
      onChanged();
    });

  const forgetWorkspace = (id: string) =>
    withBusy(`forget:${id}`, async () => {
      await api.forgetWorkspace(id);
      onChanged();
    });

  const missingByWorkspace = new Map<
    string,
    { branch: string; repos: string[] }
  >();
  for (const m of missing) {
    const entry = missingByWorkspace.get(m.workspace_id) ?? {
      branch: m.branch,
      repos: [],
    };
    entry.repos.push(m.repo_key);
    missingByWorkspace.set(m.workspace_id, entry);
  }

  return (
    <section>
      <h4>State / disk mismatch</h4>
      <p className="muted">
        Tethys found things that don&apos;t line up between{" "}
        <code>state.json</code> and your <code>worktree_root</code>. Usually the
        result of a crash or manual filesystem surgery.
      </p>

      {orphans.length > 0 && (
        <>
          <h4>Orphaned worktrees</h4>
          <p className="muted">
            Directories with no matching workspace in state. Safe to remove.
          </p>
          <ul className="status-list">
            {orphans.map((o) => {
              const working = busy.has(`orphan:${o.path}`);
              return (
                <li key={o.path}>
                  <div className="status-row">
                    <code>{o.path}</code>
                  </div>
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
          <h4>Missing worktrees</h4>
          <p className="muted">
            Workspaces in state whose worktree directories have vanished. Forget
            drops the workspace from state; it does not touch disk.
          </p>
          <ul className="status-list">
            {Array.from(missingByWorkspace.entries()).map(
              ([id, { branch, repos }]) => {
                const working = busy.has(`forget:${id}`);
                return (
                  <li key={id}>
                    <div className="status-row">
                      <code>{branch}</code>{" "}
                      <span className="muted">· missing: {repos.join(", ")}</span>
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
    </section>
  );
}

function Spinner() {
  return <span className="spinner" aria-hidden="true" />;
}

function formatRemaining(ms: number): string {
  if (ms <= 0) return "now";
  const minutes = Math.ceil(ms / 60000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rem = minutes % 60;
  return rem === 0 ? `${hours}h` : `${hours}h ${rem}m`;
}
