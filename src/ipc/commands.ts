import type { TethysBridge } from "./bridge";
import type {
  Agent,
  Artifact,
  BudgetRequest,
  CreateWorkspaceArgs,
  Discrepancies,
  Folder,
  FolderId,
  GithubAuthSnapshot,
  GithubPrStatus,
  JobEvent,
  PrRef,
  RegistryEdit,
  RegistryStatus,
  SessionInfo,
  SystemErrorEntry,
  Theme,
  Workspace,
  WorkspaceId,
} from "../types";

// Read per call, so fun mode's preview can install its stand-in after import.
const bridge = (): TethysBridge => window.tethys;

/** A stream the daemon writes to, passed to a command as its id. */
export class Channel<T> {
  readonly id: number;
  onmessage: (message: T) => void = () => {};

  constructor() {
    this.id = bridge().openChannel((m) => this.onmessage(m as T));
  }

  /** The daemon never drops a channel on its own; detach first. */
  close() {
    bridge().closeChannel(this.id);
  }
}

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const wire =
    args &&
    Object.fromEntries(
      Object.entries(args).map(([k, v]) => [
        k,
        v instanceof Channel ? v.id : v,
      ]),
    );
  return bridge().invoke(cmd, wire) as Promise<T>;
}

/** http(s) URLs only. */
export const openUrl = (url: string) => bridge().openUrl(url);

/** Daemon broadcasts; returns the unlisten function. */
export const listen = (event: string, handler: (payload: unknown) => void) =>
  bridge().listen(event, handler);

export const pathForFile = (file: File) => bridge().pathForFile(file);

/** Served by `electron/pages.ts`, which only reads under `worktree_root`. Each
 *  segment is escaped on its own so a page's relative links still resolve. */
export const convertFileSrc = (path: string) =>
  "tethys-file://local" + path.split("/").map(encodeURIComponent).join("/");

/** Each wrapper fixes its command's calling convention (flat camelCase or a
 *  snake_case `args` struct), which TypeScript can't check at call sites. */

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

/** An empty `prs` ships the workspace's only open PR. */
export const startShipping = (workspaceId: WorkspaceId, prs: PrRef[]) =>
  invoke<void>("start_shipping", {
    args: { workspace_id: workspaceId, prs },
  });

export const stopShipping = (workspaceId: WorkspaceId) =>
  invoke<void>("stop_shipping", { workspaceId });

export const listArtifacts = (workspaceId: WorkspaceId) =>
  invoke<Artifact[]>("list_artifacts", { workspaceId });

export const dismissArtifact = (workspaceId: WorkspaceId, artifactId: string) =>
  invoke<void>("dismiss_artifact", { workspaceId, artifactId });

export const openArtifact = (workspaceId: WorkspaceId, artifactId: string) =>
  invoke<void>("open_artifact", { workspaceId, artifactId });

export const openInVscode = (id: WorkspaceId) =>
  invoke<void>("open_in_vscode", { id });

export const listFolders = () => invoke<Folder[]>("list_folders");

export const createFolder = (name: string) =>
  invoke<Folder>("create_folder", { name });

export const renameFolder = (folderId: FolderId, name: string) =>
  invoke<void>("rename_folder", { args: { folder_id: folderId, name } });

export const deleteFolder = (id: FolderId) =>
  invoke<void>("delete_folder", { id });

export const setFolderCollapsed = (folderId: FolderId, collapsed: boolean) =>
  invoke<void>("set_folder_collapsed", {
    args: { folder_id: folderId, collapsed },
  });

export const reorderFolders = (ids: FolderId[]) =>
  invoke<void>("reorder_folders", { ids });

export const moveWorkspacesToFolder = (
  workspaceIds: WorkspaceId[],
  folder: FolderId | null,
) =>
  invoke<void>("move_workspaces_to_folder", {
    args: { workspace_ids: workspaceIds, folder },
  });

export const getSession = (workspaceId: WorkspaceId) =>
  invoke<SessionInfo | null>("get_session", { workspaceId });

export const startAgentSession = (workspaceId: WorkspaceId) =>
  invoke<SessionInfo>("start_agent_session", { workspaceId });

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

export const listBudgetRequests = () =>
  invoke<BudgetRequest[]>("list_budget_requests");

export const decideBudgetRequest = (
  workspaceId: WorkspaceId,
  approve: boolean,
) => invoke<void>("decide_budget_request", { workspaceId, approve });

/** Scrollback arrives as `onBytes`' first chunk, ahead of live bytes. */
export const attachSession = (
  sessionId: string,
  onBytes: Channel<Uint8Array>,
) => invoke<null>("attach_session", { sessionId, onBytes });

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

/** Rejects when `repos.toml` doesn't load; the registry in use is kept. */
export const reloadRegistry = () => invoke<RegistryStatus>("reload_registry");

export const saveRegistry = (edit: RegistryEdit) =>
  invoke<RegistryStatus>("save_registry", { edit });

export const defaultWorkspaceDoc = () =>
  invoke<string>("default_workspace_doc");

export const listDiscrepancies = () =>
  invoke<Discrepancies>("list_discrepancies");

export const listSystemErrors = () =>
  invoke<SystemErrorEntry[]>("list_system_errors");

export const dismissSystemError = (id: string) =>
  invoke<void>("dismiss_system_error", { id });

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

export const createWorkspace = (
  args: CreateWorkspaceArgs,
  onEvent: Channel<JobEvent>,
) => invoke<Workspace>("create_workspace", { args, onEvent });

export const addRepoToWorkspace = (
  workspaceId: WorkspaceId,
  repoKeys: string[],
  onEvent: Channel<JobEvent>,
) =>
  invoke<void>("add_repo_to_workspace", {
    args: { workspace_id: workspaceId, repo_keys: repoKeys },
    onEvent,
  });
