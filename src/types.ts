export type WorkspaceId = string;
export type SessionId = string;

export type SessionRuntimeState =
  | "dormant"
  | "working"
  | "waiting_input"
  | "idle";

export type PrState = "open" | "merged" | "closed";

export type ChecksRollup =
  | "none"
  | "pending"
  | "success"
  | "failure"
  | "neutral";

export type ReviewDecision =
  | "none"
  | "approved"
  | "changes_requested"
  | "review_required";

export interface GithubPrStatus {
  pr_number: number;
  url: string;
  state: PrState;
  is_draft: boolean;
  checks: ChecksRollup;
  /** Cursor Bugbot's check, split out from `checks` for its own indicator. */
  bugbot: ChecksRollup;
  /** GitHub reports the PR conflicts with its base branch. Surfaced through the CI indicator. */
  has_merge_conflicts: boolean;
  review_decision: ReviewDecision;
  unresolved_threads: number;
  head_sha: string;
  fetched_at: string;
  last_error: string | null;
}

export interface RepoLink {
  repo_key: string;
  worktree_path: string;
  setup_script_ran_at: string | null;
  github: GithubPrStatus | null;
}

export type SessionKind = "claude" | "frontend_build" | "backend_build";

export interface ClaudeSessionMeta {
  id: SessionId;
  /** What process this session was spawned for. Defaults to "claude" on
   *  pre-existing state entries from before the field was added. */
  kind?: SessionKind;
  /** `null` => session is rooted at the workspace dir (parent of all repo worktrees). */
  repo_key: string | null;
  cwd: string;
  claude_session_id: string | null;
  transcript_path: string | null;
  /** Cosmetic: when true the session is filtered out of the chip bar
   *  unless the user toggles "show hidden". The tmux session keeps running. */
  hidden: boolean;
  /** User-set chip label override. `null`/missing falls back to the
   *  default (first 8 chars of `id`). Set via the chip's right-click
   *  Rename menu. */
  display_name?: string | null;
}

export interface DevServersMeta {
  fe_port: number;
  /** `null` when only the FE is running (FE-only mode — branch had no
   *  backend changes, or user explicitly skipped). */
  be_port: number | null;
  fe_session_id: string | null;
  be_session_id: string | null;
  /** What `NL_PROXY_TARGET` (or equivalent) was set to when the FE
   *  spawned. Lets the UI show "FE → master" vs "FE → this worktree". */
  fe_proxy_target: string;
  started_at: string;
}

export type BeMode = "auto" | "force_include" | "force_exclude";

export type WorkspaceStatus =
  | { kind: "ready" }
  | { kind: "creating" }
  | { kind: "creation_failed"; error: string };

export interface Workspace {
  id: WorkspaceId;
  branch: string;
  created_at: string;
  repo_links: RepoLink[];
  sessions: ClaudeSessionMeta[];
  /** Override the claude entry-point binary name for sessions in this workspace
   *  (e.g. `claude-hipaa`). `null` falls back to the default `claude`. */
  claude_binary: string | null;
  /** Soft-delete marker. The workspace is hidden from the sidebar until the
   *  hourly purger runs (only purges entries older than 1 hour). */
  deleted_at: string | null;
  /** Archive marker. Archived workspaces render in a collapsed group at
   *  the bottom of the sidebar. */
  archived_at: string | null;
  /** Lifecycle state. `creating` rows render as a spinner row in the sidebar
   *  and a JobLogPane in the detail; `creation_failed` rows render the
   *  failed log so the user can read the error before dismissing. */
  status: WorkspaceStatus;
  /** User-pinned chip order. `null`/missing falls back to the default
   *  newest-first display. Any session id in `sessions` but not in
   *  this list is appended on render in its existing order, so new
   *  sessions don't have to be retroactively inserted. */
  session_order?: string[] | null;
  /** Set when dev servers are running for this workspace. Persisted so the
   *  UI strip survives Tethys restarts (memory poller reconciles against
   *  actual container/process state on its next tick). */
  dev_servers: DevServersMeta | null;
}

export interface SystemErrorEntry {
  id: string;
  at: string;
  kind: string;
  message: string;
  workspace_id: string | null;
  workspace_branch: string | null;
}

export interface CreateWorkspaceArgs {
  /** Frontend-minted UUID. Used so the backend can insert the workspace
   *  draft into state immediately and the sidebar row holds its position. */
  workspace_id: WorkspaceId;
  branch: string;
  repo_selections: string[];
  claude_binary?: string | null;
}

export interface Repo {
  key: string;
  remote_url: string;
  default_setup_script: string | null;
  setup_timeout_secs: number | null;
}

export type RegistryStatus =
  | { kind: "ok"; path: string; registry: { worktree_root: string; repos: Repo[] } }
  | { kind: "missing"; path: string }
  | { kind: "invalid"; path: string; error: string };

export type JobEvent =
  | { kind: "status"; message: string; repo?: string }
  | { kind: "log"; stream: "stdout" | "stderr"; line: string; repo?: string }
  | { kind: "success" }
  | { kind: "failed"; error: string };

export interface OrphanedDir {
  path: string;
}

export interface MissingWorktree {
  workspace_id: string;
  branch: string;
  repo_key: string;
  worktree_path: string;
}

export interface Discrepancies {
  orphaned_dirs: OrphanedDir[];
  missing_worktrees: MissingWorktree[];
}

export interface SessionInfo {
  id: string;
  workspace_id: string;
  /** `null` => session is rooted at the workspace dir (parent of all repo worktrees). */
  repo_key: string | null;
  cwd: string;
  running: boolean;
  runtime_state: SessionRuntimeState;
  notification_type: string | null;
  /** User dismissed the "your turn" indicator; reset on next state transition. */
  turn_acknowledged: boolean;
}

export interface TurnChangedEvent {
  workspace_id: string;
  session_id: string;
  runtime_state: SessionRuntimeState;
  notification_type: string | null;
  turn_acknowledged: boolean;
}

export interface GithubStatusChangedEvent {
  workspace_id: string;
  repo_key: string;
  /** null when the PR no longer exists (branch unpushed or deleted). */
  status: GithubPrStatus | null;
}

export type MemoryPressure = "normal" | "warning" | "critical" | "unknown";

export interface SystemMemory {
  level: MemoryPressure;
  free_pct: number;
  free_mib: number;
  /** Total physical memory in MiB. Constant across ticks. */
  total_mib: number;
}

export interface WorkspaceMemory {
  workspace_id: string;
  fe_mib: number;
  be_mib: number;
}

export interface MemorySnapshot {
  system: SystemMemory;
  per_workspace: WorkspaceMemory[];
}

export interface ServiceLiveState {
  session_id: string | null;
  running: boolean;
  port: number | null;
}

export interface DevStateSnapshot {
  workspace_id: WorkspaceId;
  fe: ServiceLiveState | null;
  be: ServiceLiveState | null;
  fe_proxy_target: string | null;
}

export type GithubAuthState =
  | "unknown"
  | "authenticated"
  | "not_authenticated"
  | "disabled";

export interface GithubAuthSnapshot {
  state: GithubAuthState;
  login: string | null;
}

export interface ThemeColors {
  background: string;
  foreground: string;
  cursor: string;
  cursor_text: string;
  selection: string;
  /** 16 ANSI colors, `ansi[0]` = black, `ansi[1]` = red, etc. */
  ansi: string[];
}

export interface Theme {
  name: string;
  source_path: string;
  colors: ThemeColors;
}
