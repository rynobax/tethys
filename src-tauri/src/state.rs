use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::github::GithubPrStatus;

pub type WorkspaceId = String;
pub type FolderId = String;
pub type SessionId = String;
pub type ScriptRunId = String;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// User-created folders, in the order the sidebar draws them. Ordering
    /// lives in the Vec, exactly as it does for `workspaces`.
    ///
    /// The Default folder is deliberately not in here: it *is* the absence of
    /// a folder (`Workspace::folder == None`), which is what makes it always
    /// present, unnameable, and impossible to delete.
    #[serde(default)]
    pub folders: Vec<Folder>,
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
    /// Which folder this workspace sits in; `None` is the Default folder.
    ///
    /// Purely organisational — a workspace behaves identically wherever it
    /// sits. Stored per workspace rather than as a list on the folder so that
    /// membership has exactly one home, which is also why purging a workspace
    /// needs no folder bookkeeping.
    ///
    /// A folder id that no longer resolves is pruned to `None` at boot, so a
    /// hand-edited `state.json` naming a stranger lands in Default instead of
    /// dropping the row out of the sidebar entirely.
    #[serde(default)]
    pub folder: Option<FolderId>,
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

/// A named place in the sidebar that holds workspaces.
///
/// Flat — folders never contain folders — and inert: membership decides where
/// a row is drawn and nothing else. It replaced the archive marker, which was
/// the same idea wearing behaviour it didn't need.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    /// Whether the sidebar hides this folder's rows. Persisted, unlike the
    /// archive drawer's expand state that came before it: with one drawer,
    /// forgetting was fine, but the folder you're working out of should still
    /// be open after a restart.
    #[serde(default)]
    pub collapsed: bool,
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            collapsed: false,
        }
    }
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
    /// Asked for, but waiting its turn in the provisioning queue — nothing on
    /// disk yet and no process running for it. Distinct from `Creating` only
    /// so the sidebar can tell the truth about which one of a batch is
    /// actually being built; both are drafts, and both are pruned at boot.
    Queued,
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
    /// Every PR this repo link tracks, in the order tracking started.
    ///
    /// One list, deliberately: the PR for the workspace's own branch used to
    /// live in a slot of its own, which bought it four behaviours nothing else
    /// had — it couldn't be detached, it re-derived itself from the branch
    /// every tick, it vanished silently instead of showing "no data", and
    /// re-pointing at it was a refresh where re-attaching anything else was an
    /// error. None of that was worth the split. The branch PR is now just the
    /// one entry that gets *added* for you (see `TargetKind::Branch`); past
    /// that it is an ordinary tracked PR.
    #[serde(default)]
    pub prs: Vec<TrackedPr>,
    /// PR numbers detached from this link, so branch discovery doesn't put
    /// them straight back. The price of letting the automatically-added PR be
    /// detached like any other: without this, the next poll re-adds it.
    ///
    /// Manually attaching a dismissed number clears it — asking for a PR by
    /// name outranks having once said no to it.
    #[serde(default)]
    pub dismissed: Vec<u32>,
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

/// A pull request a repo link tracks. The number is the intent and persists
/// even when a poll fails; `status` is the last successful fetch (`None` until
/// the first one lands, or if the PR became unreachable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedPr {
    pub number: u32,
    pub tracked_at: DateTime<Utc>,
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
        folder: Option<FolderId>,
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
            folder,
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

impl RepoLink {
    pub fn tracked(&self, number: u32) -> Option<&TrackedPr> {
        self.prs.iter().find(|p| p.number == number)
    }

    pub fn tracked_mut(&mut self, number: u32) -> Option<&mut TrackedPr> {
        self.prs.iter_mut().find(|p| p.number == number)
    }

    /// Start tracking `number`, or refresh what we already have for it.
    ///
    /// Idempotent for both callers — the attach dialog re-pasting a number and
    /// the poller re-finding the branch PR mean the same thing here, which is
    /// the point of there being one list. Tracking a number also un-dismisses
    /// it, so a detached PR you ask for again comes back and stays.
    pub fn track(&mut self, number: u32, status: Option<GithubPrStatus>) {
        self.dismissed.retain(|n| *n != number);
        match self.tracked_mut(number) {
            // A refresh with nothing to say (a failed fetch) leaves the last
            // known status alone rather than blanking a good chip.
            Some(existing) => {
                if status.is_some() {
                    existing.status = status;
                }
            }
            None => self.prs.push(TrackedPr {
                number,
                tracked_at: Utc::now(),
                status,
            }),
        }
    }

    /// Stop tracking `number` and remember the refusal, so branch discovery
    /// doesn't re-add it on the next tick. Returns whether it was tracked.
    pub fn untrack(&mut self, number: u32) -> bool {
        let had = self.tracked(number).is_some();
        self.prs.retain(|p| p.number != number);
        if !self.dismissed.contains(&number) {
            self.dismissed.push(number);
        }
        had
    }

    /// Whether branch discovery should leave `number` alone: either we already
    /// track it, or it was detached.
    pub fn discovery_should_skip(&self, number: u32) -> bool {
        self.tracked(number).is_some() || self.dismissed.contains(&number)
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

    /// True when two workspaces sit in different folders.
    ///
    /// A blocker link across that boundary can never draw — nesting only
    /// happens within a folder — so it is refused at the door rather than
    /// stored as a pointer with no visible effect. A workspace that isn't in
    /// state counts as different from everything, which errs towards refusing.
    pub fn folders_differ(&self, a: &str, b: &str) -> bool {
        let folder_of = |id: &str| self.find_workspace(id).map(|w| w.folder.clone());
        folder_of(a) != folder_of(b)
    }

    pub fn find_folder_mut(&mut self, id: &str) -> Option<&mut Folder> {
        self.folders.iter_mut().find(|f| f.id == id)
    }

    pub fn folder_exists(&self, id: &str) -> bool {
        self.folders.iter().any(|f| f.id == id)
    }

    /// Move every workspace in `folder_id` to Default. Used when the folder
    /// is deleted: contents fall back rather than the delete being refused.
    pub fn empty_folder(&mut self, folder_id: &str) {
        for ws in &mut self.workspaces {
            if ws.folder.as_deref() == Some(folder_id) {
                ws.folder = None;
            }
        }
    }

    /// Send workspaces naming a folder that isn't there back to Default,
    /// returning how many moved.
    ///
    /// `state.json` is hand-editable, so this is the same shape of tolerance
    /// as the cycle hop limit: a file that breaks the invariant still has to
    /// boot.
    pub fn prune_missing_folders(&mut self) -> usize {
        let known: Vec<FolderId> = self.folders.iter().map(|f| f.id.clone()).collect();
        let mut moved = 0;
        for ws in &mut self.workspaces {
            if let Some(id) = &ws.folder {
                if !known.contains(id) {
                    ws.folder = None;
                    moved += 1;
                }
            }
        }
        moved
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
                    prs: Vec::new(),
                    dismissed: Vec::new(),
                    created_branch: true,
                })
                .collect(),
            sessions: Vec::new(),
            claude_binary: None,
            origin: Origin::Ui,
            deleted_at: None,
            folder: None,
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
        // Old RepoLink JSON without `prs` deserializes to an empty list. The
        // retired `github` / `attached_prs` slots are folded in by
        // `Store::load`, not by serde.
        assert!(ws.repo_links[0].prs.is_empty());
        assert!(ws.repo_links[0].dismissed.is_empty());
        assert!(ws.claude_binary.is_none());
        assert!(ws.deleted_at.is_none());
        assert!(ws.folder.is_none());
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
                        None,
                    );
                    ws.blocked_by = blocker.map(str::to_string);
                    ws
                })
                .collect(),
            ..Default::default()
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
    fn tracked_prs_round_trip() {
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
                            "prs": [
                                {
                                    "number": 512,
                                    "tracked_at": "2026-04-02T09:00:00Z",
                                    "status": null
                                }
                            ],
                            "dismissed": [7]
                        }
                    ]
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        let link = &parsed.workspaces[0].repo_links[0];
        assert_eq!(link.prs.len(), 1);
        assert_eq!(link.prs[0].number, 512);
        assert!(link.prs[0].status.is_none());
        assert_eq!(link.dismissed, vec![7]);
    }

    fn link() -> RepoLink {
        RepoLink {
            repo_key: "frontend".into(),
            worktree_path: PathBuf::from("/tmp/wt/frontend"),
            setup_script_ran_at: None,
            prs: Vec::new(),
            dismissed: Vec::new(),
            created_branch: true,
        }
    }

    #[test]
    fn tracking_twice_refreshes_rather_than_duplicates() {
        let mut l = link();
        l.track(7, None);
        l.track(7, None);
        assert_eq!(l.prs.len(), 1);
    }

    /// A failed refetch must not blank a chip that already has good data.
    #[test]
    fn refresh_with_no_status_keeps_the_last_one() {
        let mut l = link();
        l.track(7, Some(pr_status(7)));
        l.track(7, None);
        assert_eq!(l.prs[0].status.as_ref().unwrap().pr_number, 7);
    }

    #[test]
    fn detaching_dismisses_so_discovery_skips_it() {
        let mut l = link();
        l.track(7, Some(pr_status(7)));
        assert!(l.untrack(7));
        assert!(l.prs.is_empty());
        assert!(l.discovery_should_skip(7));
    }

    /// Asking for a PR by number outranks having once said no to it.
    #[test]
    fn tracking_a_dismissed_number_un_dismisses_it() {
        let mut l = link();
        l.untrack(7);
        l.track(7, Some(pr_status(7)));
        assert!(l.dismissed.is_empty());
        assert!(!l.discovery_should_skip(9));
    }

    fn pr_status(number: u32) -> GithubPrStatus {
        GithubPrStatus {
            pr_number: number,
            url: format!("https://github.com/o/r/pull/{number}"),
            state: crate::github::status::PrState::Open,
            is_draft: false,
            checks: crate::github::status::ChecksRollup::None,
            bugbot: crate::github::status::ChecksRollup::None,
            has_merge_conflicts: false,
            review_decision: crate::github::status::ReviewDecision::None,
            unresolved_threads: 0,
            head_branch: None,
            head_sha: String::new(),
            stack: None,
            merge_queue: None,
            fetched_at: Utc::now(),
            last_error: None,
        }
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

    fn folder_state(members: &[(&str, Option<&str>)]) -> AppState {
        AppState {
            workspaces: members
                .iter()
                .map(|(id, folder)| {
                    Workspace::draft(
                        (*id).into(),
                        format!("branch/{id}"),
                        None,
                        Origin::Ui,
                        folder.map(str::to_string),
                    )
                })
                .collect(),
            folders: vec![
                Folder {
                    id: "f1".into(),
                    name: "Later".into(),
                    collapsed: false,
                },
                Folder {
                    id: "f2".into(),
                    name: "Archived".into(),
                    collapsed: true,
                },
            ],
            ..Default::default()
        }
    }

    /// The Default folder is the *absence* of a folder, so two unfiled
    /// workspaces are in the same one — not merely both unfiled.
    #[test]
    fn default_folder_counts_as_a_folder_for_blocking() {
        let s = folder_state(&[("a", None), ("b", None), ("c", Some("f1"))]);
        assert!(!s.folders_differ("a", "b"));
        assert!(s.folders_differ("a", "c"));
    }

    /// Errs towards refusing: a blocker that isn't in state can't be shown to
    /// share a folder with anything.
    #[test]
    fn a_missing_workspace_differs_from_everything() {
        let s = folder_state(&[("a", None)]);
        assert!(s.folders_differ("a", "ghost"));
    }

    #[test]
    fn deleting_a_folder_sends_its_contents_to_default() {
        let mut s = folder_state(&[("a", Some("f1")), ("b", Some("f1")), ("c", Some("f2"))]);
        s.empty_folder("f1");
        let filed: Vec<Option<String>> = s.workspaces.iter().map(|w| w.folder.clone()).collect();
        assert_eq!(filed, vec![None, None, Some("f2".to_string())]);
    }

    #[test]
    fn pruning_only_moves_workspaces_whose_folder_is_gone() {
        let mut s = folder_state(&[("a", Some("f1")), ("b", Some("stranger")), ("c", None)]);
        assert_eq!(s.prune_missing_folders(), 1);
        let filed: Vec<Option<String>> = s.workspaces.iter().map(|w| w.folder.clone()).collect();
        assert_eq!(filed, vec![Some("f1".to_string()), None, None]);
    }
}
