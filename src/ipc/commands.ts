import { Channel, invoke } from "@tauri-apps/api/core";

/**
 * Re-exported so this module is genuinely the only file importing from
 * `@tauri-apps/api/core` — the rule is easier to keep than "only `invoke`
 * comes from there, `Channel` is fine".
 */
export { Channel };

import type {
  Discrepancies,
  Folder,
  FolderId,
  GithubAuthSnapshot,
  GithubPrStatus,
  JobEvent,
  PendingPermission,
  RegistryStatus,
  ScriptInfo,
  SessionInfo,
  SystemErrorEntry,
  Theme,
  Workspace,
  WorkspaceId,
} from "../types";

/**
 * The one place that talks to the Tauri command layer.
 *
 * Two things this buys, neither of which is depth — the wrappers are one line
 * over one line, and that is fine, because what's wanted here is a *seam*:
 *
 * 1. It is the only importer of `@tauri-apps/api/core`. Six files used to
 *    import `invoke` directly, which meant no component in this app could be
 *    rendered in a test at all. Swap this module and they can be.
 *
 * 2. The calling convention is stated once per command instead of being
 *    remembered at every call site. Tauri commands here come in two
 *    incompatible shapes — flat auto-camelCased arguments, and a wrapped
 *    `args:` struct with snake_case fields inside — and nothing marked which
 *    was which. Both appeared ten lines apart in the same function. Get it
 *    wrong and TypeScript says nothing; you get a rejected promise at
 *    runtime.
 *
 * `scripts/check-ipc-parity.mjs` asserts these names match the Rust
 * `#[tauri::command]` set, so a rename on that side fails here instead of at
 * runtime.
 */

// ── workspaces ─────────────────────────────────────────────────────────────

export const listWorkspaces = () => invoke<Workspace[]>("list_workspaces");

export const reorderWorkspaces = (ids: WorkspaceId[]) =>
  invoke<void>("reorder_workspaces", { ids });

export const deleteWorkspace = (id: WorkspaceId) =>
  invoke<void>("delete_workspace", { id });

export const cancelDeleteWorkspace = (id: WorkspaceId) =>
  invoke<void>("cancel_delete_workspace", { id });

export const forgetWorkspace = (id: WorkspaceId) =>
  invoke<void>("forget_workspace", { id });

export const setWorkspaceNotes = (workspaceId: WorkspaceId, notes: string) =>
  invoke<void>("set_workspace_notes", {
    args: { workspace_id: workspaceId, notes },
  });

/** `blockerId: null` clears the link. Rejects on a cycle. */
export const setWorkspaceBlocker = (
  workspaceId: WorkspaceId,
  blockerId: WorkspaceId | null,
) =>
  invoke<void>("set_workspace_blocker", {
    args: { workspace_id: workspaceId, blocker_id: blockerId },
  });

export const openInVscode = (id: WorkspaceId) =>
  invoke<void>("open_in_vscode", { id });

// ── folders ────────────────────────────────────────────────────────────────

export const listFolders = () => invoke<Folder[]>("list_folders");

export const createFolder = (name: string) =>
  invoke<Folder>("create_folder", { name });

export const renameFolder = (folderId: FolderId, name: string) =>
  invoke<void>("rename_folder", { args: { folder_id: folderId, name } });

/** Contents fall back to the Default folder. */
export const deleteFolder = (id: FolderId) =>
  invoke<void>("delete_folder", { id });

export const setFolderCollapsed = (folderId: FolderId, collapsed: boolean) =>
  invoke<void>("set_folder_collapsed", {
    args: { folder_id: folderId, collapsed },
  });

export const reorderFolders = (ids: FolderId[]) =>
  invoke<void>("reorder_folders", { ids });

/** `folder: null` files them into Default. A blocker stack moves as a unit,
 *  so this normally carries every id in the stack at once. */
export const moveWorkspacesToFolder = (
  workspaceIds: WorkspaceId[],
  folder: FolderId | null,
) =>
  invoke<void>("move_workspaces_to_folder", {
    args: { workspace_ids: workspaceIds, folder },
  });

// ── claude sessions ────────────────────────────────────────────────────────

export const listSessions = (workspaceId: WorkspaceId) =>
  invoke<SessionInfo[]>("list_sessions", { workspaceId });

export const startClaudeSession = (
  workspaceId: WorkspaceId,
  repoKey: string | null,
) =>
  invoke<SessionInfo>("start_claude_session", {
    args: { workspace_id: workspaceId, repo_key: repoKey },
  });

export const resumeClaudeSession = (
  workspaceId: WorkspaceId,
  repoKey: string | null,
  sessionMetaId: string,
) =>
  invoke<SessionInfo>("resume_claude_session", {
    args: {
      workspace_id: workspaceId,
      repo_key: repoKey,
      session_meta_id: sessionMetaId,
    },
  });

export const switchClaudeBinary = (
  workspaceId: WorkspaceId,
  sessionMetaId: string,
  claudeBinary: string,
) =>
  invoke<SessionInfo>("switch_claude_binary", {
    args: {
      workspace_id: workspaceId,
      session_meta_id: sessionMetaId,
      claude_binary: claudeBinary,
    },
  });

export const setClaudeSessionHidden = (
  workspaceId: WorkspaceId,
  sessionId: string,
  hidden: boolean,
) =>
  invoke<void>("set_claude_session_hidden", {
    args: { workspace_id: workspaceId, session_id: sessionId, hidden },
  });

export const acknowledgeSessionTurn = (
  workspaceId: WorkspaceId,
  sessionId: string,
) => invoke<void>("acknowledge_session_turn", { workspaceId, sessionId });

// ── session pty ────────────────────────────────────────────────────────────

export const attachSession = (
  sessionId: string,
  onBytes: Channel<ArrayBuffer>,
) => invoke<number[]>("attach_session", { sessionId, onBytes });

export const detachSession = (sessionId: string, channelId: number) =>
  invoke<void>("detach_session", { sessionId, channelId });

export const sendInput = (sessionId: string, data: number[]) =>
  invoke<void>("send_input", { sessionId, data });

export const resizeSession = (sessionId: string, cols: number, rows: number) =>
  invoke<void>("resize_session", { sessionId, cols, rows });

// ── scripts ────────────────────────────────────────────────────────────────

export const listScripts = (workspaceId: WorkspaceId) =>
  invoke<ScriptInfo[]>("list_scripts", { workspaceId });

export const startScript = (
  workspaceId: WorkspaceId,
  repoKey: string,
  scriptName: string,
) =>
  invoke<ScriptInfo>("start_script", {
    args: {
      workspace_id: workspaceId,
      repo_key: repoKey,
      script_name: scriptName,
    },
  });

export const dismissScript = (workspaceId: WorkspaceId, scriptId: string) =>
  invoke<void>("dismiss_script", {
    args: { workspace_id: workspaceId, script_id: scriptId },
  });

export const attachScript = (
  scriptId: string,
  onBytes: Channel<ArrayBuffer>,
) => invoke<number[]>("attach_script", { scriptId, onBytes });

export const detachScript = (scriptId: string, channelId: number) =>
  invoke<void>("detach_script", { scriptId, channelId });

export const sendInputScript = (scriptId: string, data: number[]) =>
  invoke<void>("send_input_script", { scriptId, data });

export const resizeScript = (scriptId: string, cols: number, rows: number) =>
  invoke<void>("resize_script", { scriptId, cols, rows });

// ── github ─────────────────────────────────────────────────────────────────

export const githubAuthStatus = () =>
  invoke<GithubAuthSnapshot>("github_auth_status");

export const githubReprobeAuth = () =>
  invoke<GithubAuthSnapshot>("github_reprobe_auth");

export const attachPr = (
  workspaceId: WorkspaceId,
  repoKey: string | null,
  reference: string,
) =>
  invoke<GithubPrStatus>("attach_pr", {
    args: { workspace_id: workspaceId, repo_key: repoKey, reference },
  });

export const detachPr = (
  workspaceId: WorkspaceId,
  repoKey: string,
  prNumber: number,
) =>
  invoke<void>("detach_pr", {
    args: {
      workspace_id: workspaceId,
      repo_key: repoKey,
      pr_number: prNumber,
    },
  });

// ── registry / system status ───────────────────────────────────────────────

export const registryStatus = () => invoke<RegistryStatus>("registry_status");

export const listDiscrepancies = () =>
  invoke<Discrepancies>("list_discrepancies");

export const listSystemErrors = () =>
  invoke<SystemErrorEntry[]>("list_system_errors");

export const dismissSystemError = (id: string) =>
  invoke<void>("dismiss_system_error", { id });

export const listPendingPermissions = () =>
  invoke<PendingPermission[]>("list_pending_permissions");

export const applyPendingPermission = (id: string, targetRepoKeys: string[]) =>
  invoke<void>("apply_pending_permission", {
    args: { id, target_repo_keys: targetRepoKeys },
  });

export const dismissPendingPermission = (id: string) =>
  invoke<void>("dismiss_pending_permission", { id });

export const removeOrphanDir = (path: string) =>
  invoke<void>("remove_orphan_dir", { path });

export const runPurgeNow = () => invoke<void>("run_purge_now");

export const openReposConfig = () => invoke<void>("open_repos_config");

// ── misc ───────────────────────────────────────────────────────────────────

export const getTheme = () => invoke<Theme | null>("get_theme");

export const readClipboardFilePaths = () =>
  invoke<string[]>("read_clipboard_file_paths");

// ── long-running jobs ──────────────────────────────────────────────────────

/**
 * Commands that stream `JobEvent`s over a channel while they run. Driven by
 * `useBackendJob`, which needs the name and args as data rather than as a
 * call — hence the descriptor shape rather than a plain function.
 */
export const jobs = {
  createWorkspace: (args: Record<string, unknown>) => ({
    command: "create_workspace" as const,
    args,
  }),
  addRepoToWorkspace: (args: Record<string, unknown>) => ({
    command: "add_repo_to_workspace" as const,
    args,
  }),
};

export const runJob = (
  command: string,
  args: Record<string, unknown>,
  onEvent: Channel<JobEvent>,
) => invoke<unknown>(command, { ...args, onEvent });
