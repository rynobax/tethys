use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::github::GithubPrStatus;

pub type WorkspaceId = String;
pub type SessionId = String;
pub type ScriptRunId = String;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// Errors raised by the background purger when it failed to tear down
    /// a soft-deleted workspace. Surfaced in the system status modal.
    #[serde(default)]
    pub system_errors: Vec<SystemErrorEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub branch: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub repo_links: Vec<RepoLink>,
    #[serde(default)]
    pub sessions: Vec<ClaudeSessionMeta>,
    /// Override the entry-point binary name for sessions in this workspace
    /// (e.g. `claude-hipaa`). `None` falls back to the app-wide `claude`
    /// resolved at boot.
    #[serde(default)]
    pub claude_binary: Option<String>,
    /// Soft-delete marker. When set, the workspace is hidden from the
    /// sidebar and queued for the hourly purger. Cleared by
    /// `cancel_delete_workspace` to undo before the cron runs.
    #[serde(default)]
    pub deleted_at: Option<DateTime<Utc>>,
    /// Archive marker. Archived workspaces render in the collapsed
    /// "Archived" section at the bottom of the sidebar.
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
    /// Lifecycle state of the workspace itself. Newly-submitted entries land
    /// in state as `Creating` so the sidebar row appears at the user's
    /// chosen position from t=0; provisioning then flips it to `Ready` (or
    /// `CreationFailed` with the error message). Persisted as `Ready` for
    /// every pre-existing workspace via the field default.
    #[serde(default)]
    pub status: WorkspaceStatus,
    /// Running script processes (started via the per-repo `scripts` registry
    /// entries). Persisted so they can be reattached after a Tethys restart.
    /// Removed when a script exits or the user stops it.
    #[serde(default)]
    pub script_runs: Vec<ScriptRunMeta>,
}

/// A live script process attached to a workspace+repo. The Tethys `id` is
/// also the tmux session name on the Tethys server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptRunMeta {
    pub id: ScriptRunId,
    /// Repo this script was configured under. Used to look up the command
    /// in the registry on reattach and to label the chip.
    pub repo_key: String,
    /// Key from `Repo.scripts` — the user-facing name (e.g. `dev`).
    pub script_name: String,
    /// Command string at start time. Cached so the chip still labels itself
    /// correctly if the user edits the registry after starting the script.
    pub command: String,
    pub cwd: PathBuf,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceStatus {
    #[default]
    Ready,
    Creating,
    CreationFailed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemErrorEntry {
    pub id: String,
    pub at: DateTime<Utc>,
    /// Free-form category for grouping in the UI (e.g. "purge").
    pub kind: String,
    pub message: String,
    /// Optional workspace context — set when the error refers to a
    /// specific workspace (e.g. the soft-deleted one we failed to purge).
    #[serde(default)]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default)]
    pub workspace_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoLink {
    pub repo_key: String,
    pub worktree_path: PathBuf,
    pub setup_script_ran_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub github: Option<GithubPrStatus>,
    /// Whether Tethys created this branch (branched off HEAD or off a remote
    /// tracking ref) versus checked out a branch that already existed locally.
    /// Teardown only deletes branches Tethys created, so checking out a
    /// pre-existing PR branch never destroys it. Defaults to `true` for state
    /// written before this field existed — those branches were always created
    /// by Tethys under the old branch pre-check.
    #[serde(default = "default_created_branch")]
    pub created_branch: bool,
}

fn default_created_branch() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeSessionMeta {
    pub id: SessionId,
    /// `None` => session was started at the workspace root (the parent dir
    /// containing each repo's worktree subdir), not inside any one repo.
    #[serde(default)]
    pub repo_key: Option<String>,
    pub cwd: PathBuf,
    pub claude_session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    /// User-set: when true, the session chip is filtered out of the
    /// default chip bar. The tmux session and supervisor handle stay
    /// live — hide is purely cosmetic.
    #[serde(default)]
    pub hidden: bool,
    /// Last turn state observed via Claude Code hooks. Persisted so the
    /// "your turn" indicator survives Tethys restarts. `None` until the
    /// first hook lands (or for state.json from before this field existed).
    #[serde(default)]
    pub runtime_state: Option<SessionRuntimeState>,
    /// Notification subtype that accompanied the last `WaitingInput`
    /// transition (e.g. `permission_prompt`). Cleared when the session
    /// leaves `WaitingInput`.
    #[serde(default)]
    pub notification_type: Option<String>,
    /// User dismissed the "your turn" indicator for this session via the
    /// sidebar context menu. Reset to `false` on the next `runtime_state`
    /// transition (a state change is the user-facing signal that something
    /// fresh happened, so the dot should re-light). Persisted so the
    /// dismissal survives a Tethys restart.
    #[serde(default)]
    pub turn_acknowledged: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionRuntimeState {
    /// PTY not running (session has never been spawned, or was spawned and exited).
    #[default]
    Dormant,
    /// PTY running and actively processing (Claude is thinking, or user just typed).
    Working,
    /// Claude finished responding, no explicit input prompt up — default "nothing pending" state.
    Idle,
    /// Claude is blocked on user input — either the main prompt or a permission dialog.
    WaitingInput,
}

impl AppState {
    pub fn find_workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn find_workspace_mut(&mut self, id: &str) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_github_state_json_round_trips() {
        // This is the shape of state.json from before the `github` field was
        // added to RepoLink. It must still deserialize cleanly.
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [
                        {
                            "repo_key": "frontend",
                            "worktree_path": "/tmp/wt/abc-123/frontend",
                            "setup_script_ran_at": null
                        }
                    ]
                }
            ]
        }"#;

        let parsed: AppState = serde_json::from_str(raw).expect("old state.json must deserialize");
        assert_eq!(parsed.workspaces.len(), 1);
        let ws = &parsed.workspaces[0];
        assert_eq!(ws.id, "abc-123");
        assert_eq!(ws.branch, "feat/foo");
        assert_eq!(ws.repo_links.len(), 1);
        assert!(ws.repo_links[0].github.is_none());
        assert!(ws.claude_binary.is_none());
        assert!(ws.deleted_at.is_none());
        assert!(ws.archived_at.is_none());
        assert!(parsed.system_errors.is_empty());
    }

    #[test]
    fn pre_turn_state_session_round_trips() {
        // ClaudeSessionMeta from before runtime_state/notification_type were
        // added must still deserialize.
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "sessions": [
                        {
                            "id": "sess-1",
                            "cwd": "/tmp/wt/abc-123/frontend",
                            "claude_session_id": null,
                            "transcript_path": null
                        }
                    ]
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        let session = &parsed.workspaces[0].sessions[0];
        assert!(session.runtime_state.is_none());
        assert!(session.notification_type.is_none());
        assert!(!session.turn_acknowledged);
    }

    #[test]
    fn claude_binary_round_trips() {
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "claude_binary": "claude-hipaa"
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        assert_eq!(
            parsed.workspaces[0].claude_binary.as_deref(),
            Some("claude-hipaa")
        );
    }

    #[test]
    fn pre_status_state_defaults_to_ready() {
        // state.json from before the WorkspaceStatus field was added must
        // load as Ready — older entries are by definition fully-provisioned.
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z"
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        assert!(matches!(parsed.workspaces[0].status, WorkspaceStatus::Ready));
    }

    #[test]
    fn pre_script_runs_state_defaults_to_empty() {
        // Workspaces persisted before script_runs was added must load with
        // an empty Vec.
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z"
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        assert!(parsed.workspaces[0].script_runs.is_empty());
    }

    #[test]
    fn workspace_status_round_trips() {
        let failed = WorkspaceStatus::CreationFailed {
            error: "boom".into(),
        };
        let bytes = serde_json::to_vec(&failed).expect("serialize");
        let back: WorkspaceStatus = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(failed, back);
    }
}
