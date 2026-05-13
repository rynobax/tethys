use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::github::GithubPrStatus;

pub type WorkspaceId = String;
pub type SessionId = String;

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
    /// User-pinned session chip order. `None` falls back to the default
    /// ordering (newest first via `sessions.reverse()` in the UI).
    /// Once the user manually drags a chip, we persist the resulting
    /// order here so it survives Tethys restarts. Any session id that
    /// appears in `sessions` but not in this list is appended to the
    /// end on render — new sessions don't have to be retroactively
    /// inserted into the override.
    #[serde(default)]
    pub session_order: Option<Vec<SessionId>>,
    /// Dev-server state for this workspace (FE + BE servers + their ports).
    /// `None` when no dev servers are running. Persisted so the strip survives
    /// a Tethys restart — at boot we reconcile against actual docker/process
    /// state via the memory poller.
    #[serde(default)]
    pub dev_servers: Option<DevServersMeta>,
}

/// Per-workspace dev-server state. Populated by `start_dev_servers` and
/// cleared by `stop_dev_servers`. The session ids point into `sessions`
/// (each tagged `kind: FrontendBuild` / `BackendBuild`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevServersMeta {
    pub fe_port: u16,
    /// `None` if the BE wasn't started (FE-only mode — branch had no
    /// backend changes, or RAM-tight start).
    #[serde(default)]
    pub be_port: Option<u16>,
    pub fe_session_id: Option<SessionId>,
    pub be_session_id: Option<SessionId>,
    /// What `NL_PROXY_TARGET` (or equivalent) was set to when FE spawned.
    /// Useful for diagnosing "why is my FE hitting master?" — distinguishes
    /// "auto-decided FE-only" from "BE failed to start so FE fell back".
    pub fe_proxy_target: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeSessionMeta {
    pub id: SessionId,
    /// What kind of session this is. Defaults to `Claude` so pre-existing
    /// state.json entries (which lack the field) deserialize as the only
    /// kind that existed before.
    #[serde(default)]
    pub kind: SessionKind,
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
    /// User-set display name for the chip. `None` falls back to the
    /// default label (the first 8 chars of the Tethys session id). Set
    /// via the chip's right-click → Rename menu. Trimmed/empty values
    /// are treated as None.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// What process a session was spawned for. Adding new kinds is backward-
/// compatible because the field has `#[serde(default)]` on the struct and
/// `Claude` is the default — old state.json deserializes as the only kind
/// that ever existed before.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Claude,
    /// `yarn dev` (or equivalent FE build process) for a worktree.
    FrontendBuild,
    /// `docker compose up` (or equivalent BE process) for a worktree.
    BackendBuild,
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
    fn pre_session_kind_defaults_to_claude() {
        // ClaudeSessionMeta from before SessionKind was added must
        // deserialize as Claude (the only kind that existed before).
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
        assert_eq!(parsed.workspaces[0].sessions[0].kind, SessionKind::Claude);
        assert!(parsed.workspaces[0].dev_servers.is_none());
    }

    #[test]
    fn dev_servers_round_trips() {
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "sessions": [
                        {
                            "id": "tethys-fe-abc-123",
                            "kind": "frontend_build",
                            "cwd": "/tmp/wt/abc-123/frontend",
                            "claude_session_id": null,
                            "transcript_path": null
                        },
                        {
                            "id": "tethys-be-abc-123",
                            "kind": "backend_build",
                            "cwd": "/tmp/wt/abc-123/backend",
                            "claude_session_id": null,
                            "transcript_path": null
                        }
                    ],
                    "dev_servers": {
                        "fe_port": 3001,
                        "be_port": 8001,
                        "fe_session_id": "tethys-fe-abc-123",
                        "be_session_id": "tethys-be-abc-123",
                        "fe_proxy_target": "http://localhost:8001",
                        "started_at": "2026-05-12T10:00:00Z"
                    }
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        let ws = &parsed.workspaces[0];
        let dev = ws.dev_servers.as_ref().expect("dev_servers should be set");
        assert_eq!(dev.fe_port, 3001);
        assert_eq!(dev.be_port, Some(8001));
        assert_eq!(ws.sessions[0].kind, SessionKind::FrontendBuild);
        assert_eq!(ws.sessions[1].kind, SessionKind::BackendBuild);
        // Round-trip through serialization too.
        let bytes = serde_json::to_vec(&parsed).expect("serialize");
        let back: AppState = serde_json::from_slice(&bytes).expect("re-deserialize");
        assert_eq!(back.workspaces[0].sessions.len(), 2);
        assert_eq!(back.workspaces[0].dev_servers.as_ref().unwrap().fe_port, 3001);
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

    #[test]
    fn pre_session_order_defaults_to_none() {
        // state.json from before session_order + display_name landed
        // must still deserialize, with both fields defaulting to None.
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
        assert!(parsed.workspaces[0].session_order.is_none());
        assert!(parsed.workspaces[0].sessions[0].display_name.is_none());
    }

    #[test]
    fn session_order_and_display_name_round_trip() {
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
                            "transcript_path": null,
                            "display_name": "code review"
                        },
                        {
                            "id": "sess-2",
                            "cwd": "/tmp/wt/abc-123/frontend",
                            "claude_session_id": null,
                            "transcript_path": null
                        }
                    ],
                    "session_order": ["sess-2", "sess-1"]
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        let ws = &parsed.workspaces[0];
        assert_eq!(
            ws.session_order.as_ref().map(|v| v.as_slice()),
            Some(&["sess-2".to_string(), "sess-1".to_string()][..])
        );
        assert_eq!(ws.sessions[0].display_name.as_deref(), Some("code review"));
        assert!(ws.sessions[1].display_name.is_none());
        // Re-serialize + re-deserialize cleanly.
        let bytes = serde_json::to_vec(&parsed).expect("serialize");
        let back: AppState = serde_json::from_slice(&bytes).expect("re-deserialize");
        assert_eq!(back.workspaces[0].session_order, ws.session_order);
    }
}
