import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type {
  Discrepancies,
  PendingPermission,
  RegistryStatus,
  SystemErrorEntry,
  Workspace,
  WorkspaceId,
} from "./types";
import { useTauriEvent } from "./useTauriEvent";

type Props = {
  /** All workspaces, including soft-deleted. The modal lists pending deletions. */
  allWorkspaces: Workspace[];
  /** Result of the last `registry_status` invoke — drives the config-path
   *  row in the Status tab. */
  registry: RegistryStatus | null;
  /** State/disk mismatches surfaced in the Status tab. */
  discrepancies: Discrepancies | null;
  /** Reload app state after a discrepancy is resolved (remove orphan / forget). */
  onDiscrepancyChange: () => void;
};

const HOUR_MS = 60 * 60 * 1000;
type TabId = "status" | "pending_permissions";

export function SystemStatus({
  allWorkspaces,
  registry,
  discrepancies,
  onDiscrepancyChange,
}: Props) {
  const [errors, setErrors] = useState<SystemErrorEntry[]>([]);
  const [pending, setPending] = useState<PendingPermission[]>([]);
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<TabId>("status");

  const refreshErrors = useCallback(async () => {
    try {
      const list = await invoke<SystemErrorEntry[]>("list_system_errors");
      setErrors(list);
    } catch (e) {
      console.error("list_system_errors:", e);
    }
  }, []);

  const refreshPending = useCallback(async () => {
    try {
      const list = await invoke<PendingPermission[]>("list_pending_permissions");
      setPending(list);
    } catch (e) {
      console.error("list_pending_permissions:", e);
    }
  }, []);

  useEffect(() => {
    refreshErrors();
    refreshPending();
  }, [refreshErrors, refreshPending]);

  useTauriEvent("system_status:changed", () => refreshErrors());
  useTauriEvent("pending_permissions:changed", () => refreshPending());

  const pendingDeletes = allWorkspaces.filter((w) => w.deleted_at !== null);
  const hasErrors = errors.length > 0;
  const hasPending = pending.length > 0;
  const hasDiscrepancies =
    (discrepancies?.orphaned_dirs.length ?? 0) > 0 ||
    (discrepancies?.missing_worktrees.length ?? 0) > 0;
  // Yellow only signals something actionable: pending permission grants or a
  // state/disk mismatch. Pending deletions are routine and don't warrant it.
  const hasWarnings = hasPending || hasDiscrepancies;

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
            hasErrors ? "red" : hasWarnings ? "yellow" : "green"
          }`}
        />
        Status
      </button>
      {open && (
        <SystemStatusModal
          tab={tab}
          onTabChange={setTab}
          errors={errors}
          pendingDeletes={pendingDeletes}
          pendingPermissions={pending}
          discrepancies={discrepancies}
          registry={registry}
          onClose={() => setOpen(false)}
          onRefreshErrors={refreshErrors}
          onRefreshPending={refreshPending}
          onDiscrepancyChange={onDiscrepancyChange}
        />
      )}
    </>
  );
}

function SystemStatusModal({
  tab,
  onTabChange,
  errors,
  pendingDeletes,
  pendingPermissions,
  discrepancies,
  registry,
  onClose,
  onRefreshErrors,
  onRefreshPending,
  onDiscrepancyChange,
}: {
  tab: TabId;
  onTabChange: (t: TabId) => void;
  errors: SystemErrorEntry[];
  pendingDeletes: Workspace[];
  pendingPermissions: PendingPermission[];
  discrepancies: Discrepancies | null;
  registry: RegistryStatus | null;
  onClose: () => void;
  onRefreshErrors: () => void;
  onRefreshPending: () => void;
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
        <div className="modal-tabs">
          <button
            type="button"
            className={`modal-tab${tab === "status" ? " active" : ""}`}
            onClick={() => onTabChange("status")}
          >
            Status
          </button>
          <button
            type="button"
            className={`modal-tab${tab === "pending_permissions" ? " active" : ""}`}
            onClick={() => onTabChange("pending_permissions")}
          >
            Pending permissions
            {pendingPermissions.length > 0 && (
              <span className="tab-badge">{pendingPermissions.length}</span>
            )}
          </button>
        </div>

        {tab === "status" ? (
          <StatusTab
            errors={errors}
            pendingDeletes={pendingDeletes}
            discrepancies={discrepancies}
            registry={registry}
            onRefresh={onRefreshErrors}
            onDiscrepancyChange={onDiscrepancyChange}
          />
        ) : (
          <PendingPermissionsTab
            entries={pendingPermissions}
            onRefresh={onRefreshPending}
          />
        )}

        <div className="modal-actions">
          <button type="button" onClick={onClose} autoFocus>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

function StatusTab({
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

  const cancelDelete = async (id: WorkspaceId) => {
    setBusy(`cancel:${id}`);
    try {
      await invoke("cancel_delete_workspace", { id });
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
    }
  };

  const dismissError = async (id: string) => {
    setBusy(`err:${id}`);
    try {
      await invoke("dismiss_system_error", { id });
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
    }
  };

  const runCleanupNow = async () => {
    setBusy("cleanup");
    try {
      await invoke("run_purge_now");
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
      onRefresh();
    }
  };

  const openConfig = async () => {
    try {
      await invoke("open_repos_config");
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
        <div className="section-header">
          <h4>Configuration</h4>
          <button type="button" onClick={openConfig}>
            Open repos.toml
          </button>
        </div>
        {registry ? (
          <ul className="status-list">
            <li>
              <div className="status-row">
                <span className="muted">repos.toml</span>
                <code>{registry.path}</code>
              </div>
            </li>
            {registry.kind === "ok" && (
              <li>
                <div className="status-row">
                  <span className="muted">worktree_root</span>
                  <code>{registry.registry.worktree_root}</code>
                </div>
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

function PendingPermissionsTab({
  entries,
  onRefresh,
}: {
  entries: PendingPermission[];
  onRefresh: () => void;
}) {
  return (
    <section>
      <div className="section-header">
        <h4>Captured workspace-root grants</h4>
      </div>
      {entries.length === 0 ? (
        <p className="muted">
          No pending entries. When a workspace is purged, any permissions
          Claude approved at the workspace-root level — that aren&apos;t already
          in the per-repo settings — get captured here for review.
        </p>
      ) : (
        <ul className="status-list pending-permissions-list">
          {entries.map((entry) => (
            <PendingPermissionRow
              key={entry.id}
              entry={entry}
              onChanged={onRefresh}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

function PendingPermissionRow({
  entry,
  onChanged,
}: {
  entry: PendingPermission;
  onChanged: () => void;
}) {
  const initialSelection = useMemo(() => {
    const set = new Set<string>();
    if (entry.suggested_repo_key) set.add(entry.suggested_repo_key);
    return set;
  }, [entry.suggested_repo_key]);
  const [selected, setSelected] = useState<Set<string>>(initialSelection);
  const [busy, setBusy] = useState(false);

  const toggle = (key: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const apply = async () => {
    if (selected.size === 0) return;
    setBusy(true);
    try {
      await invoke("apply_pending_permission", {
        args: {
          id: entry.id,
          target_repo_keys: Array.from(selected),
        },
      });
      onChanged();
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(false);
    }
  };

  const dismiss = async () => {
    setBusy(true);
    try {
      await invoke("dismiss_pending_permission", { id: entry.id });
      onChanged();
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(false);
    }
  };

  const displayEntry =
    entry.suggested_repo_key &&
    selected.size === 1 &&
    selected.has(entry.suggested_repo_key) &&
    entry.stripped_entry
      ? entry.stripped_entry
      : entry.raw_entry;

  return (
    <li className="pending-permission-row">
      <div className="status-row">
        <span className={`permission-category ${entry.category}`}>
          {entry.category}
        </span>
        <code className="permission-entry">{displayEntry}</code>
      </div>
      <div className="muted permission-meta">
        from <code>{entry.workspace_branch}</code> ·{" "}
        {new Date(entry.captured_at).toLocaleString()}
      </div>
      <div className="permission-repo-picker">
        <span className="muted">Apply to:</span>
        {entry.workspace_repo_keys.length === 0 ? (
          <span className="muted">(no repos recorded)</span>
        ) : (
          entry.workspace_repo_keys.map((key) => (
            <label key={key} className="permission-repo-option">
              <input
                type="checkbox"
                checked={selected.has(key)}
                onChange={() => toggle(key)}
                disabled={busy}
              />
              {key}
              {key === entry.suggested_repo_key && (
                <span className="muted"> (suggested)</span>
              )}
            </label>
          ))
        )}
      </div>
      <div className="permission-actions">
        <button
          type="button"
          onClick={apply}
          disabled={busy || selected.size === 0}
        >
          Apply
        </button>
        <button type="button" onClick={dismiss} disabled={busy}>
          Dismiss
        </button>
      </div>
    </li>
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
