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
  /** The PR's head branch. Tells manually-attached PRs on the same repo apart.
   *  `null` for statuses persisted before this field existed. */
  head_branch: string | null;
  /** Where the PR sits in a `gh stack`, if it's in one. `null` both for a lone
   *  PR and for PRs merely based on each other by hand. */
  stack: PrStack | null;
  head_sha: string;
  fetched_at: string;
  last_error: string | null;
}

/** A PR's membership in a GitHub stack. A PR is in at most one. */
export interface PrStack {
  /** Identifies the stack within its repository — stack-mates share it. */
  number: number;
  /** Total PRs in the stack, including any this workspace doesn't track. */
  size: number;
  /** This PR's slot, where 1 is closest to the stack's base branch. */
  position: number;
}

/** A PR a repo link tracks, however it came to be tracked. */
export interface TrackedPr {
  number: number;
  tracked_at: string;
  /** `null` until the first successful fetch, or if the PR became unreachable. */
  status: GithubPrStatus | null;
}

export interface RepoLink {
  repo_key: string;
  worktree_path: string;
  setup_script_ran_at: string | null;
  /** Every PR this link tracks, in the order tracking started. The PR for the
   *  workspace's own branch is in here like any other — the poller adds it for
   *  you, and that is the only way it differs from one you attached by hand. */
  prs: TrackedPr[];
  /** PR numbers detached from this link, so the poller's branch scan doesn't
   *  put them back. Nothing in the UI reads this; it's here because it round
   *  trips through `workspace:changed`. */
  dismissed: number[];
  docs?: { branch: string; checkout_path: string; linked_paths: string[] } | null;
}

export interface ClaudeSessionMeta {
  id: SessionId;
  /** `null` => session is rooted at the workspace dir (parent of all repo worktrees). */
  repo_key: string | null;
  cwd: string;
  claude_session_id: string | null;
  transcript_path: string | null;
  /** Per-session override for the claude entry-point binary. Takes precedence
   *  over the workspace's `claude_binary`. `null` falls back to it. */
  claude_binary: string | null;
  /** Cosmetic: when true the session is filtered out of the chip bar
   *  unless the user toggles "show hidden". The tmux session keeps running. */
  hidden: boolean;
}

/** A named place in the sidebar holding workspaces. Flat, and purely
 *  organisational — membership decides where a row is drawn, nothing else.
 *  The Default folder is not one of these: it's `Workspace.folder === null`,
 *  which is what makes it always present and impossible to rename or delete. */
export interface Folder {
  id: FolderId;
  name: string;
  /** Persisted, so the folder you work out of stays open across restarts. */
  collapsed: boolean;
}

export type WorkspaceStatus =
  | { kind: "ready" }
  /** Asked for, but waiting its turn: Tethys provisions one workspace at a
   *  time so a batch of setup scripts doesn't starve each other out. */
  | { kind: "queued" }
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
  /** Which folder the workspace sits in; `null` is the Default folder. */
  folder: FolderId | null;
  /** Lifecycle state. `queued` and `creating` rows both render as pending
   *  rows in the sidebar with a JobLogPane in the detail, and only `creating`
   *  spins; `creation_failed` rows render the failed log so the user can read
   *  the error before dismissing. */
  status: WorkspaceStatus;
  /** Freeform user notes, edited via the notes overlay in the detail pane.
   *  Empty string when unset. */
  notes: string;
  /** The workspace this one is waiting on. A pointer, not a state — whether
   *  it counts as blocked depends on the blocker still being on screen, which
   *  is why `workspaceTree` decides that and not this field. */
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
  /** Frontend-minted UUID. Used so the backend can insert the workspace
   *  draft into state immediately and the sidebar row holds its position. */
  workspace_id: WorkspaceId;
  branch: string;
  repo_selections: string[];
  claude_binary?: string | null;
  /** Folder the new workspace lands in; `null`/absent is Default. */
  folder?: FolderId | null;
}

export interface Repo {
  key: string;
  remote_url: string;
  default_branch: string | null;
  default_setup_script: string | null;
  setup_timeout_secs: number | null;
  copy_files: string[];
  /** Named shell commands runnable inside a workspace's worktree of this repo
   *  (e.g. `{ "dev": "yarn dev" }`). */
  scripts: { [name: string]: string };
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
  /**
   * Whether this session wants the user's attention. Derived in Rust from
   * running + runtime_state + turn_acknowledged, so every consumer agrees —
   * the frontend used to recompute it in four places from two different
   * definitions.
   */
  needs_turn: boolean;
  /** Whether Claude is actively working. Derived alongside `needs_turn`. */
  working: boolean;
}

export interface ScriptInfo {
  id: string;
  workspace_id: string;
  repo_key: string;
  script_name: string;
  command: string;
  cwd: string;
  running: boolean;
  started_at: string;
}

export interface TurnChangedEvent {
  workspace_id: string;
  session_id: string;
  runtime_state: SessionRuntimeState;
  notification_type: string | null;
  turn_acknowledged: boolean;
  /** Whether the session's PTY is still alive. */
  running: boolean;
  /** Same derived predicates as on `SessionInfo`, so both paths agree. */
  needs_turn: boolean;
  working: boolean;
}

export interface GithubStatusChangedEvent {
  workspace_id: string;
  repo_key: string;
  /** Always names the PR the status belongs to — there is no longer a slot to
   *  disambiguate. */
  pr_number: number;
  /** null when the PR no longer exists (branch unpushed or deleted). */
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
  /** 16 ANSI colors, `ansi[0]` = black, `ansi[1]` = red, etc. */
  ansi: string[];
}

export interface Theme {
  name: string;
  source_path: string;
  colors: ThemeColors;
}
