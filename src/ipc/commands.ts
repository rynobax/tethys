import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";

/**
 * Re-exported so this module is genuinely the only file importing from
 * `@tauri-apps/api/core` — the rule is easier to keep than "only `invoke`
 * comes from there, `Channel` is fine". `convertFileSrc` turns a path into
 * an `asset://` URL the Page iframe can load.
 */
export { Channel, convertFileSrc };

import type {
  Artifact,
  Discrepancies,
  Folder,
  FolderId,
  GithubAuthSnapshot,
  GithubPrStatus,
  JobEvent,
  PendingPermission,
  RegistryStatus,
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

export const listArtifacts = (workspaceId: WorkspaceId) =>
  invoke<Artifact[]>("list_artifacts", { workspaceId });

export const dismissArtifact = (workspaceId: WorkspaceId, artifactId: string) =>
  invoke<void>("dismiss_artifact", { workspaceId, artifactId });

/** Open a Page artifact in the default browser. */
export const openArtifact = (workspaceId: WorkspaceId, artifactId: string) =>
  invoke<void>("open_artifact", { workspaceId, artifactId });

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

// ── the workspace's claude session ─────────────────────────────────────────

/** `null` when the workspace's session is dormant (or never started). */
export const getSession = (workspaceId: WorkspaceId) =>
  invoke<SessionInfo | null>("get_session", { workspaceId });

/** Reattach, resume, or start fresh — whichever the session's state calls
 *  for. The one call behind Start, Resume and Reconnect alike. */
export const startClaudeSession = (workspaceId: WorkspaceId) =>
  invoke<SessionInfo>("start_claude_session", { workspaceId });

/** Change the workspace's binary and restart its session under it, keeping
 *  the conversation when there is one on disk. */
export const switchClaudeBinary = (
  workspaceId: WorkspaceId,
  claudeBinary: string,
) =>
  invoke<SessionInfo>("switch_claude_binary", {
    args: { workspace_id: workspaceId, claude_binary: claudeBinary },
  });

export const acknowledgeSessionTurn = (workspaceId: WorkspaceId) =>
  invoke<void>("acknowledge_session_turn", { workspaceId });

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

/** The Tethys-owned locations the Configuration panel can open. */
export type ConfigLocation = "repos_config" | "worktree_root" | "clone_dir";

export const openConfigLocation = (location: ConfigLocation) =>
  invoke<void>("open_config_location", { location });

export const cloneDirPath = () => invoke<string>("clone_dir_path");

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
