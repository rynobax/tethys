import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";

/** Re-exported so this stays the only importer of `@tauri-apps/api/core`. */
export { Channel, convertFileSrc };

import type {
  Agent,
  Artifact,
  CreateWorkspaceArgs,
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
 * The seam over the Tauri command layer. Each wrapper fixes its command's
 * calling convention — flat camelCased arguments or a wrapped snake_case
 * `args` struct — which TypeScript can't check at the call site.
 */

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

export const openArtifact = (workspaceId: WorkspaceId, artifactId: string) =>
  invoke<void>("open_artifact", { workspaceId, artifactId });

/** `rect` is in viewport coordinates; Rust needs `viewportHeight` to map them
 *  into the window's content view. */
export const showPrView = (
  url: string,
  rect: { x: number; y: number; width: number; height: number },
  viewportHeight: number,
) => invoke<void>("show_pr_view", { url, ...rect, viewportHeight });

export const hidePrView = () => invoke<void>("hide_pr_view");

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

/** `folder: null` is Default. */
export const moveWorkspacesToFolder = (
  workspaceIds: WorkspaceId[],
  folder: FolderId | null,
) =>
  invoke<void>("move_workspaces_to_folder", {
    args: { workspace_ids: workspaceIds, folder },
  });

export const getSession = (workspaceId: WorkspaceId) =>
  invoke<SessionInfo | null>("get_session", { workspaceId });

/** Reattach, resume, or start fresh — whichever the session's state calls for. */
export const startAgentSession = (workspaceId: WorkspaceId) =>
  invoke<SessionInfo>("start_agent_session", { workspaceId });

/** Switching across agents starts a fresh conversation; neither CLI can read
 *  the other's transcript. */
export const switchAgent = (
  workspaceId: WorkspaceId,
  agent: Agent,
  agentBinary: string,
) =>
  invoke<SessionInfo>("switch_agent", {
    args: {
      workspace_id: workspaceId,
      agent,
      agent_binary: agentBinary,
    },
  });

export const acknowledgeSessionTurn = (workspaceId: WorkspaceId) =>
  invoke<void>("acknowledge_session_turn", { workspaceId });

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

export type ConfigLocation = "repos_config" | "worktree_root" | "clone_dir";

export const openConfigLocation = (location: ConfigLocation) =>
  invoke<void>("open_config_location", { location });

export const cloneDirPath = () => invoke<string>("clone_dir_path");

export const getTheme = () => invoke<Theme | null>("get_theme");

export const readClipboardFilePaths = () =>
  invoke<string[]>("read_clipboard_file_paths");

/** Descriptors rather than calls: `useBackendJob` needs the command name and
 *  args as data. */
export const jobs = {
  createWorkspace: (args: { args: CreateWorkspaceArgs }) => ({
    command: "create_workspace" as const,
    args,
  }),
  addRepoToWorkspace: (args: {
    args: { workspace_id: string; repo_key: string };
  }) => ({
    command: "add_repo_to_workspace" as const,
    args,
  }),
};

export const runJob = (
  command: string,
  args: Record<string, unknown>,
  onEvent: Channel<JobEvent>,
) => invoke<unknown>(command, { ...args, onEvent });
