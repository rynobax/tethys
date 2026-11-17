import { useEffect, useMemo, useState } from "react";
import * as api from "./ipc/commands";

import type { RegistryEdit, RegistryStatus, Repo } from "./types";

/** Everything in `repos.toml`, edited as form fields and saved back to it. */
export function Settings({ registry }: { registry: RegistryStatus | null }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        className="system-status-button"
        onClick={() => setOpen(true)}
        title="Settings"
      >
        <span className="settings-icon" aria-hidden="true">
          ⚙
        </span>
        Settings
      </button>
      {open && (
        <SettingsModal fallback={registry} onClose={() => setOpen(false)} />
      )}
    </>
  );
}

type RepoDraft = {
  /** React's key: `key` itself is typed into for a new repo. */
  id: string;
  isNew: boolean;
  expanded: boolean;
  key: string;
  enabled: boolean;
  remoteUrl: string;
  defaultBranch: string;
  setupScript: string;
  teardownScript: string;
  timeout: string;
  copyFiles: string;
  env: string;
  notes: string;
};

type Draft = {
  worktreeRoot: string;
  repos: RepoDraft[];
  workspaceDoc: string;
};

type Loaded =
  | { kind: "loading" }
  | { kind: "ready"; draft: Draft; problem: string | null }
  | { kind: "unusable"; path: string; error: string };

function SettingsModal({
  fallback,
  onClose,
}: {
  /** What the app already had, for when the file won't reload. */
  fallback: RegistryStatus | null;
  onClose: () => void;
}) {
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [defaultDoc, setDefaultDoc] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.defaultWorkspaceDoc().then(setDefaultDoc, () => {});
    const show = (status: RegistryStatus | null, problem: string | null) => {
      if (status?.kind === "invalid") {
        setLoaded({ kind: "unusable", path: status.path, error: status.error });
        return;
      }
      const d = draftFrom(status);
      setLoaded({ kind: "ready", draft: d, problem });
      setDraft(d);
    };
    api.reloadRegistry().then(
      (status) => show(status, null),
      (e) => show(fallback, String(e)),
    );
    // Loaded once per opening; later changes to `fallback` are the save's own.
  }, []);

  const initial = loaded.kind === "ready" ? loaded.draft : null;
  const dirty = useMemo(
    () =>
      !!draft &&
      !!initial &&
      JSON.stringify(strip(draft)) !== JSON.stringify(strip(initial)),
    [draft, initial],
  );

  const close = () => {
    if (!dirty) onClose();
  };

  const updateRepo = (id: string, patch: Partial<RepoDraft>) =>
    setDraft(
      (d) =>
        d && {
          ...d,
          repos: d.repos.map((r) => (r.id === id ? { ...r, ...patch } : r)),
        },
    );

  const addRepo = () =>
    setDraft(
      (d) =>
        d && {
          ...d,
          repos: [
            ...d.repos,
            {
              ...emptyRepo(),
              id: crypto.randomUUID(),
              isNew: true,
              expanded: true,
            },
          ],
        },
    );

  const removeRepo = (id: string) =>
    setDraft((d) => d && { ...d, repos: d.repos.filter((r) => r.id !== id) });

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!draft) return;
    const built = toEdit(draft);
    if (typeof built === "string") {
      setError(built);
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await api.saveRegistry(built);
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={close}>
      <form
        className="modal settings-modal"
        onSubmit={save}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <h3>Settings</h3>

        {loaded.kind === "loading" && <p className="muted">Loading…</p>}

        {loaded.kind === "unusable" && (
          <>
            <p className="settings-problem">{loaded.error}</p>
            <p className="muted">
              Fix <code>{loaded.path}</code> by hand and reopen this.
            </p>
          </>
        )}

        {loaded.kind === "ready" && draft && (
          <>
            {loaded.problem && (
              <p className="settings-problem">{loaded.problem}</p>
            )}

            <section>
              <h4>Worktrees</h4>
              <label>
                Root
                <input
                  value={draft.worktreeRoot}
                  onChange={(e) =>
                    setDraft({ ...draft, worktreeRoot: e.target.value })
                  }
                  placeholder="/Users/me/code/worktrees"
                  spellCheck={false}
                />
              </label>
            </section>

            <section>
              <div className="section-header">
                <h4>Repos</h4>
                <button type="button" onClick={addRepo}>
                  Add repo
                </button>
              </div>
              {draft.repos.length === 0 && (
                <p className="muted">No repos yet.</p>
              )}
              <ul className="settings-repos">
                {draft.repos.map((r) => (
                  <RepoFields
                    key={r.id}
                    repo={r}
                    onChange={(patch) => updateRepo(r.id, patch)}
                    onRemove={() => removeRepo(r.id)}
                  />
                ))}
              </ul>
            </section>

            <section>
              <div className="section-header">
                <h4>Workspace CLAUDE.md</h4>
                {draft.workspaceDoc.trim() ? (
                  <button
                    type="button"
                    onClick={() => setDraft({ ...draft, workspaceDoc: "" })}
                  >
                    Use the built-in text
                  </button>
                ) : (
                  <button
                    type="button"
                    disabled={!defaultDoc}
                    onClick={() =>
                      setDraft({ ...draft, workspaceDoc: defaultDoc })
                    }
                  >
                    Customize
                  </button>
                )}
              </div>
              {draft.workspaceDoc.trim() ? (
                <textarea
                  className="mono"
                  rows={12}
                  value={draft.workspaceDoc}
                  onChange={(e) =>
                    setDraft({ ...draft, workspaceDoc: e.target.value })
                  }
                  spellCheck={false}
                />
              ) : (
                <p className="muted">Using the built-in text.</p>
              )}
            </section>
          </>
        )}

        {error && <p className="settings-problem">{error}</p>}

        <div className="modal-actions">
          <button type="button" onClick={onClose}>
            {dirty ? "Discard" : "Close"}
          </button>
          {loaded.kind === "ready" && (
            <button
              type="submit"
              className="primary"
              disabled={!dirty || saving}
            >
              {saving ? "Saving…" : "Save"}
            </button>
          )}
        </div>
      </form>
    </div>
  );
}

function RepoFields({
  repo,
  onChange,
  onRemove,
}: {
  repo: RepoDraft;
  onChange: (patch: Partial<RepoDraft>) => void;
  onRemove: () => void;
}) {
  const text = (field: keyof RepoDraft) => ({
    value: repo[field] as string,
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
      onChange({ [field]: e.target.value }),
    spellCheck: false,
  });

  return (
    <li className={`settings-repo${repo.enabled ? "" : " disabled"}`}>
      <div className="settings-repo-header">
        <button
          type="button"
          className="settings-repo-toggle"
          onClick={() => onChange({ expanded: !repo.expanded })}
          aria-expanded={repo.expanded}
        >
          <span className="chevron" aria-hidden="true">
            {repo.expanded ? "▾" : "▸"}
          </span>
          {repo.key || <span className="muted">new repo</span>}
        </button>
        <label className="settings-enabled">
          <input
            type="checkbox"
            checked={repo.enabled}
            onChange={(e) => onChange({ enabled: e.target.checked })}
          />
          Enabled
        </label>
      </div>

      {repo.expanded && (
        <div className="settings-repo-body">
          {repo.isNew && (
            <label>
              Key
              <input {...text("key")} placeholder="frontend" autoFocus />
            </label>
          )}
          <label>
            Remote URL
            <input
              {...text("remoteUrl")}
              placeholder="git@github.com:me/frontend.git"
            />
          </label>
          <label>
            Default branch
            <input {...text("defaultBranch")} placeholder="origin's default" />
          </label>
          <label>
            Setup script
            <textarea {...text("setupScript")} className="mono" rows={2} />
          </label>
          <label>
            Teardown script
            <textarea {...text("teardownScript")} className="mono" rows={2} />
          </label>
          <label>
            Script timeout in seconds
            <input {...text("timeout")} inputMode="numeric" placeholder="600" />
          </label>
          <label>
            Files to copy
            <textarea
              {...text("copyFiles")}
              className="mono"
              rows={2}
              placeholder=".env"
            />
          </label>
          <label>
            Environment
            <textarea
              {...text("env")}
              className="mono"
              rows={2}
              placeholder="COMPOSE_PROJECT_NAME={repo}-{workspace}"
            />
          </label>
          <label>
            Notes for Claude
            <textarea {...text("notes")} rows={3} />
          </label>
          <div>
            <button type="button" className="danger" onClick={onRemove}>
              Remove repo
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

function emptyRepo(): Omit<RepoDraft, "id" | "isNew" | "expanded"> {
  return {
    key: "",
    enabled: true,
    remoteUrl: "",
    defaultBranch: "",
    setupScript: "",
    teardownScript: "",
    timeout: "",
    copyFiles: "",
    env: "",
    notes: "",
  };
}

function draftFrom(status: RegistryStatus | null): Draft {
  if (status?.kind !== "ok") {
    return { worktreeRoot: "", repos: [], workspaceDoc: "" };
  }
  const { registry } = status;
  return {
    worktreeRoot: registry.worktree_root,
    workspaceDoc: registry.workspace_doc?.body ?? "",
    repos: registry.repos.map((r) => ({
      id: r.key,
      isNew: false,
      expanded: false,
      key: r.key,
      enabled: r.enabled,
      remoteUrl: r.remote_url,
      defaultBranch: r.default_branch ?? "",
      setupScript: r.default_setup_script ?? "",
      teardownScript: r.teardown_script ?? "",
      timeout: r.setup_timeout_secs?.toString() ?? "",
      copyFiles: r.copy_files.join("\n"),
      env: Object.entries(r.env)
        .map(([k, v]) => `${k}=${v}`)
        .join("\n"),
      notes: r.claude_notes ?? "",
    })),
  };
}

/** What counts toward "changed": not which rows happen to be open. */
function strip(draft: Draft) {
  return {
    ...draft,
    repos: draft.repos.map((r) => ({ ...r, expanded: false })),
  };
}

/** A message for the first thing that can't be saved, or the edit. */
function toEdit(draft: Draft): RegistryEdit | string {
  const worktreeRoot = draft.worktreeRoot.trim();
  if (!worktreeRoot) return "The worktree root is required.";

  const repos: Repo[] = [];
  for (const d of draft.repos) {
    const key = d.key.trim();
    const name = key || "the new repo";
    if (!key) return "Every repo needs a key.";
    if (!d.remoteUrl.trim()) return `Give ${name} a remote URL.`;

    const timeout = d.timeout.trim();
    if (timeout && !/^\d+$/.test(timeout)) {
      return `${name}'s script timeout must be a whole number of seconds.`;
    }

    const env: Record<string, string> = {};
    for (const line of lines(d.env)) {
      const eq = line.indexOf("=");
      if (eq <= 0)
        return `${name}'s environment line "${line}" needs KEY=value.`;
      env[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
    }

    repos.push({
      key,
      enabled: d.enabled,
      remote_url: d.remoteUrl.trim(),
      default_branch: orNull(d.defaultBranch.trim()),
      default_setup_script: orNull(d.setupScript.trim()),
      teardown_script: orNull(d.teardownScript.trim()),
      setup_timeout_secs: timeout ? Number(timeout) : null,
      copy_files: lines(d.copyFiles),
      env,
      claude_notes: orNull(d.notes.trimEnd()),
    });
  }

  return {
    worktree_root: worktreeRoot,
    repos,
    workspace_doc_body: orNull(draft.workspaceDoc.trimEnd()),
  };
}

function lines(s: string): string[] {
  return s
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

function orNull(s: string): string | null {
  return s.length > 0 ? s : null;
}
