use std::path::{Path, PathBuf};

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
    /// Where this workspace came from. Defaults to `Ui` for everything
    /// persisted before handoffs existed, which is what those were.
    #[serde(default)]
    pub origin: Origin,
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
    /// Freeform user notes for this workspace, edited via the notes overlay in
    /// the detail pane. Empty string when unset.
    #[serde(default)]
    pub notes: String,
    /// The workspace this one is waiting on before its own work can continue.
    ///
    /// A pointer, not a state: whether this workspace is *actually* blocked is
    /// derived from whether the blocker is still on screen, so soft-deleting or
    /// archiving the blocker frees this one without touching the field — and
    /// undoing either restores the link. It is only cleared for real where the
    /// id stops meaning anything: purge, forget, and the boot-time prune of
    /// unfinished drafts.
    #[serde(default)]
    pub blocked_by: Option<WorkspaceId>,
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

/// Who asked for this workspace. Recorded rather than displayed: a handoff is
/// a normal workspace in every respect, and the only thing the origin is for
/// is answering "where did this come from?" after the fact.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// The user, in the Tethys UI.
    #[default]
    Ui,
    /// An agent, via the handoff MCP tool.
    Handoff {
        from_workspace: WorkspaceId,
        #[serde(default)]
        from_session: Option<SessionId>,
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
    /// PRs the user manually attached to this repo link. The `github` field
    /// above only ever tracks the PR for the workspace's own branch; anything
    /// else opened from this worktree (a second branch, a stacked PR) has to
    /// be attached by hand. Polled alongside the branch PR.
    #[serde(default)]
    pub attached_prs: Vec<AttachedPr>,
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

/// A manually-attached pull request on a repo link. The number is the user's
/// intent and persists even when a poll fails; `status` is the last successful
/// fetch (`None` until the first one lands, or if the PR became unreachable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedPr {
    pub number: u32,
    pub attached_at: DateTime<Utc>,
    #[serde(default)]
    pub status: Option<GithubPrStatus>,
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
    /// Per-session override for the claude entry-point binary (e.g.
    /// `claude-hipaa`). Set when the user switches the binary for an
    /// in-progress chat; takes precedence over the workspace-level
    /// `Workspace::claude_binary`. `None` (fresh sessions, or state.json from
    /// before this field existed) falls back to the workspace default.
    #[serde(default)]
    pub claude_binary: Option<String>,
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

impl Workspace {
    /// A workspace that exists in state but not yet on disk.
    ///
    /// Both create paths insert one of these before doing any I/O, so the
    /// sidebar row appears at its final position from t=0 and — for a handoff
    /// — so the calling agent has an id to be told about. Provisioning flips
    /// the status to `Ready` or `CreationFailed` in place; the id and position
    /// never change.
    pub fn draft(
        id: WorkspaceId,
        branch: String,
        claude_binary: Option<String>,
        origin: Origin,
    ) -> Self {
        Self {
            id,
            branch,
            created_at: Utc::now(),
            repo_links: Vec::new(),
            sessions: Vec::new(),
            claude_binary,
            origin,
            deleted_at: None,
            archived_at: None,
            status: WorkspaceStatus::Creating,
            script_runs: Vec::new(),
            notes: String::new(),
            blocked_by: None,
        }
    }

    /// `<worktree_root>/<workspace_dir>` — the directory every repo worktree
    /// sits under, and the cwd for a session started at the workspace root.
    ///
    /// Derived from a repo link's parent rather than stored, so it can't drift
    /// from where the worktrees actually are.
    ///
    /// `None` when the workspace has no repo links — which is every `Creating`
    /// draft and every `CreationFailed` workspace. That is a real state, not an
    /// error: callers that need a root decide for themselves whether it means
    /// "skip", "not ready yet", or a message to the user.
    pub fn root(&self) -> Option<&Path> {
        self.repo_links
            .first()
            .and_then(|l| l.worktree_path.parent())
    }

    /// Same as [`Workspace::root`], owned — most callers pass it on to
    /// something that wants a `PathBuf`.
    pub fn root_buf(&self) -> Option<PathBuf> {
        self.root().map(Path::to_path_buf)
    }

    pub fn link(&self, repo_key: &str) -> Option<&RepoLink> {
        self.repo_links.iter().find(|r| r.repo_key == repo_key)
    }

    pub fn link_mut(&mut self, repo_key: &str) -> Option<&mut RepoLink> {
        self.repo_links.iter_mut().find(|r| r.repo_key == repo_key)
    }

    pub fn has_link(&self, repo_key: &str) -> bool {
        self.link(repo_key).is_some()
    }

    pub fn session_mut(&mut self, session_id: &str) -> Option<&mut ClaudeSessionMeta> {
        self.sessions.iter_mut().find(|m| m.id == session_id)
    }
}

impl AppState {
    pub fn find_workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn find_workspace_mut(&mut self, id: &str) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    /// True when pointing `workspace_id` at `blocker_id` would close a cycle —
    /// i.e. the proposed blocker is already waiting, directly or transitively,
    /// on the workspace being changed.
    ///
    /// The hop limit is not a performance guard. `state.json` is hand-editable
    /// and a *parse* failure there is already non-fatal, so a file carrying a
    /// pre-existing cycle has to be survivable: walking it must terminate even
    /// though the invariant this function protects was never true.
    pub fn blocker_would_cycle(&self, workspace_id: &str, blocker_id: &str) -> bool {
        if workspace_id == blocker_id {
            return true;
        }
        let mut cursor = Some(blocker_id);
        for _ in 0..self.workspaces.len() + 1 {
            let Some(id) = cursor else { return false };
            if id == workspace_id {
                return true;
            }
            cursor = self
                .find_workspace(id)
                .and_then(|w| w.blocked_by.as_deref());
        }
        // Ran out of hops with the chain still going: the existing links are
        // already cyclic. Refuse to add to them.
        true
    }

    /// Drops every link pointing at `blocker_id`. For the moments where the id
    /// stops meaning anything, as opposed to merely leaving the sidebar.
    pub fn clear_links_to(&mut self, blocker_id: &str) {
        for ws in &mut self.workspaces {
            if ws.blocked_by.as_deref() == Some(blocker_id) {
                ws.blocked_by = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_with_links(paths: &[&str]) -> Workspace {
        Workspace {
            id: "ws-1".into(),
            branch: "feat/foo".into(),
            created_at: Utc::now(),
            repo_links: paths
                .iter()
                .enumerate()
                .map(|(i, p)| RepoLink {
                    repo_key: format!("repo{i}"),
                    worktree_path: PathBuf::from(p),
                    setup_script_ran_at: None,
                    github: None,
                    attached_prs: Vec::new(),
                    created_branch: true,
                })
                .collect(),
            sessions: Vec::new(),
            claude_binary: None,
            origin: Origin::Ui,
            deleted_at: None,
            archived_at: None,
            status: WorkspaceStatus::Ready,
            script_runs: Vec::new(),
            notes: String::new(),
            blocked_by: None,
        }
    }

    /// Every repo worktree is a sibling under the workspace dir, so any link
    /// gives the same answer.
    #[test]
    fn root_is_the_parent_shared_by_every_worktree() {
        let ws = workspace_with_links(&["/wt/ws-1/frontend", "/wt/ws-1/backend"]);
        assert_eq!(ws.root(), Some(Path::new("/wt/ws-1")));
    }

    #[test]
    fn root_works_with_a_single_link() {
        let ws = workspace_with_links(&["/wt/ws-1/frontend"]);
        assert_eq!(ws.root(), Some(Path::new("/wt/ws-1")));
    }

    /// A `Creating` draft is inserted with no repo links so its sidebar row
    /// appears immediately. It has no root on disk, and every caller has to
    /// cope with that — this is the case that used to be re-decided
    /// (differently) at all seven derivation sites.
    #[test]
    fn a_workspace_with_no_repo_links_has_no_root() {
        let mut ws = workspace_with_links(&[]);
        ws.status = WorkspaceStatus::Creating;
        assert_eq!(ws.root(), None);
        assert_eq!(ws.root_buf(), None);
    }

    #[test]
    fn links_and_sessions_are_found_by_key() {
        let mut ws = workspace_with_links(&["/wt/ws-1/frontend"]);
        assert!(ws.has_link("repo0"));
        assert!(!ws.has_link("nope"));
        assert_eq!(ws.link("repo0").map(|l| l.repo_key.as_str()), Some("repo0"));
        assert!(ws.link_mut("nope").is_none());

        ws.sessions.push(ClaudeSessionMeta {
            id: "sess-1".into(),
            repo_key: None,
            cwd: PathBuf::from("/wt/ws-1"),
            claude_session_id: None,
            transcript_path: None,
            claude_binary: None,
            hidden: false,
            runtime_state: None,
            notification_type: None,
            turn_acknowledged: false,
        });
        assert!(ws.session_mut("sess-1").is_some());
        assert!(ws.session_mut("sess-2").is_none());
    }

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
        // Old RepoLink JSON without `attached_prs` deserializes to an empty list.
        assert!(ws.repo_links[0].attached_prs.is_empty());
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
        assert!(session.claude_binary.is_none());
    }

    #[test]
    fn session_claude_binary_round_trips() {
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
                            "claude_session_id": "claude-sid",
                            "transcript_path": null,
                            "claude_binary": "claude-hipaa"
                        }
                    ]
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        assert_eq!(
            parsed.workspaces[0].sessions[0].claude_binary.as_deref(),
            Some("claude-hipaa")
        );
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
    fn pre_blocked_by_state_defaults_to_unblocked() {
        // Nothing was waiting on anything before blockers existed, so the
        // absent field has to read as "no blocker" rather than failing the
        // parse — a parse failure here silently discards every workspace.
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
        assert_eq!(parsed.workspaces[0].blocked_by, None);
    }

    fn blocking_state(links: &[(&str, Option<&str>)]) -> AppState {
        AppState {
            workspaces: links
                .iter()
                .map(|(id, blocker)| {
                    let mut ws = Workspace::draft(
                        (*id).into(),
                        format!("branch/{id}"),
                        None,
                        Origin::Ui,
                    );
                    ws.blocked_by = blocker.map(str::to_string);
                    ws
                })
                .collect(),
            system_errors: Vec::new(),
        }
    }

    #[test]
    fn a_workspace_cannot_block_itself() {
        let state = blocking_state(&[("a", None)]);
        assert!(state.blocker_would_cycle("a", "a"));
    }

    #[test]
    fn an_unrelated_blocker_is_allowed() {
        // a <- b (a blocks b). Pointing c at b is a fan-out onto b's chain,
        // not a cycle.
        let state = blocking_state(&[("a", None), ("b", Some("a")), ("c", None)]);
        assert!(!state.blocker_would_cycle("c", "b"));
    }

    #[test]
    fn a_blocker_downstream_of_the_target_would_cycle() {
        // a <- b <- c. Pointing a at c would close the loop.
        let state = blocking_state(&[("a", None), ("b", Some("a")), ("c", Some("b"))]);
        assert!(state.blocker_would_cycle("a", "c"));
        assert!(state.blocker_would_cycle("a", "b"));
    }

    #[test]
    fn a_preexisting_cycle_does_not_hang_the_walk() {
        // Only reachable from a hand-edited state.json, and it has to
        // terminate rather than spin.
        let state = blocking_state(&[("a", Some("b")), ("b", Some("a")), ("c", None)]);
        assert!(state.blocker_would_cycle("c", "a"));
    }

    #[test]
    fn clearing_links_drops_every_dependent() {
        // Fan-out: one blocker, several waiting on it.
        let mut state = blocking_state(&[("a", None), ("b", Some("a")), ("c", Some("a"))]);
        state.clear_links_to("a");
        assert_eq!(state.workspaces[1].blocked_by, None);
        assert_eq!(state.workspaces[2].blocked_by, None);
    }

    #[test]
    fn pre_origin_state_defaults_to_the_ui() {
        // Every workspace persisted before handoffs existed was made by hand,
        // so the absent field has to read as `Ui` and not as "unknown".
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
        assert_eq!(parsed.workspaces[0].origin, Origin::Ui);
    }

    #[test]
    fn a_handoff_origin_round_trips() {
        let origin = Origin::Handoff {
            from_workspace: "ws-parent".into(),
            from_session: Some("sess-parent".into()),
        };
        let bytes = serde_json::to_vec(&origin).expect("serialize");
        let back: Origin = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(origin, back);
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
    fn attached_prs_round_trip() {
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
                            "setup_script_ran_at": null,
                            "attached_prs": [
                                {
                                    "number": 512,
                                    "attached_at": "2026-04-02T09:00:00Z",
                                    "status": null
                                }
                            ]
                        }
                    ]
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        let attached = &parsed.workspaces[0].repo_links[0].attached_prs;
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].number, 512);
        assert!(attached[0].status.is_none());
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
