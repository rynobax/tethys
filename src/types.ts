export type WorkspaceId = string;
export type FolderId = string;
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

export type MergeQueueState =
  | "queued"
  | "awaiting_checks"
  | "mergeable"
  | "unmergeable"
  | "locked";

export interface GithubPrStatus {
  pr_number: number;
  url: string;
  state: PrState;
  is_draft: boolean;
  checks: ChecksRollup;
  /** Cursor Bugbot's check, split out of `checks`. */
  bugbot: ChecksRollup;
  has_merge_conflicts: boolean;
  review_decision: ReviewDecision;
  /** With no verdict, tells "awaiting review" from "nobody asked". */
  review_requested: boolean;
  unresolved_threads: number;
  /** `null` for statuses persisted before this field existed. */
  head_branch: string | null;
  /** `gh stack` membership; `null` for PRs merely based on each other by hand. */
  stack: PrStack | null;
  merge_queue: MergeQueueState | null;
  head_sha: string;
  fetched_at: string;
  last_error: string | null;
}

export interface PrStack {
  number: number;
  /** Includes PRs this workspace doesn't track. */
  size: number;
  /** 1 is closest to the base branch. */
  position: number;
}

export interface TrackedPr {
  number: number;
  tracked_at: string;
  /** `null` until the first successful fetch, or once unreachable. */
  status: GithubPrStatus | null;
}

export interface RepoLink {
  repo_key: string;
  worktree_path: string;
  setup_script_ran_at: string | null;
  prs: TrackedPr[];
  /** Detached PRs, so the branch scan doesn't re-add them. */
  dismissed: number[];
  docs?: { branch: string; checkout_path: string; linked_paths: string[] } | null;
}

export type Agent = "claude" | "codex";

export interface AgentSessionMeta {
  id: SessionId;
  cwd: string;
  agent_session_id: string | null;
  transcript_path: string | null;
}

export interface Folder {
  id: FolderId;
  name: string;
  collapsed: boolean;
}

export type WorkspaceStatus =
  | { kind: "ready" }
  | { kind: "queued" }
  | { kind: "creating" }
  | { kind: "creation_failed"; error: string };

export interface Workspace {
  id: WorkspaceId;
  branch: string;
  created_at: string;
  repo_links: RepoLink[];
  session: AgentSessionMeta | null;
  agent: Agent;
  /** `null` falls back to the agent's own default binary. */
  agent_binary: string | null;
  /** Soft delete; purged once older than an hour. */
  deleted_at: string | null;
  /** `null` is the Default folder. */
  folder: FolderId | null;
  status: WorkspaceStatus;
  notes: string;
  /** Whether this counts as blocked is `workspaceTree`'s call, not this field's. */
  blocked_by: WorkspaceId | null;
}

export interface SystemErrorEntry {
  id: string;
  at: string;
  kind: string;
  message: string;
  workspace_id: string | null;
  workspace_branch: string | null;
}

export type PermissionCategory = "allow" | "deny" | "ask";

export interface PendingPermission {
  id: string;
  workspace_id: string;
  workspace_branch: string;
  workspace_repo_keys: string[];
  captured_at: string;
  category: PermissionCategory;
  raw_entry: string;
  suggested_repo_key: string | null;
  stripped_entry: string | null;
}

export interface CreateWorkspaceArgs {
  /** Frontend-minted so the draft's sidebar row holds its position. */
  workspace_id: WorkspaceId;
  branch: string;
  repo_selections: string[];
  /** `null`/absent is Claude. */
  agent?: Agent | null;
  agent_binary?: string | null;
  folder?: FolderId | null;
}

export interface Repo {
  key: string;
  remote_url: string;
  default_branch: string | null;
  default_setup_script: string | null;
  setup_timeout_secs: number | null;
  copy_files: string[];
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
  cwd: string;
  running: boolean;
  runtime_state: SessionRuntimeState;
  notification_type: string | null;
  /** Reset on the next state transition. */
  turn_acknowledged: boolean;
  /** Derived in Rust so every consumer agrees. */
  needs_turn: boolean;
  working: boolean;
  /** The program turned bracketed paste on, so the TUI will take a paste. */
  tui_ready: boolean;
}

export interface TurnChangedEvent {
  workspace_id: string;
  session_id: string;
  runtime_state: SessionRuntimeState;
  notification_type: string | null;
  turn_acknowledged: boolean;
  running: boolean;
  needs_turn: boolean;
  working: boolean;
}

export interface GithubStatusChangedEvent {
  workspace_id: string;
  repo_key: string;
  pr_number: number;
  /** null when the PR no longer exists. */
  status: GithubPrStatus | null;
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
  ansi: string[];
}

export interface Theme {
  name: string;
  source_path: string;
  colors: ThemeColors;
}

export type Artifact = {
  id: string;
  label: string;
  /** Bumped on each re-sighting so a page tab reloads in place. */
  revision: number;
} & ({ kind: "diagram"; source: string } | { kind: "page"; path: string });

export interface ArtifactChangedEvent {
  workspace_id: WorkspaceId;
  /** The artifact that arrived or was bumped; null after a dismissal. */
  artifact_id: string | null;
}
