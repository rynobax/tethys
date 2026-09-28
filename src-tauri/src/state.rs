use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::Agent;
use crate::artifacts::Artifact;
use crate::github::GithubPrStatus;

pub type WorkspaceId = String;
pub type FolderId = String;
pub type SessionId = String;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// Excludes Default, which is `Workspace::folder == None` so it can't be
    /// named or deleted.
    #[serde(default)]
    pub folders: Vec<Folder>,
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
    pub session: Option<AgentSessionMeta>,
    /// Stored, not derived from `agent_binary`: hooks, resume and MCP wiring
    /// must not hinge on how an executable is spelled.
    #[serde(default)]
    pub agent: Agent,
    /// `None` falls back to the agent's default binary.
    #[serde(default, alias = "claude_binary")]
    pub agent_binary: Option<String>,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default)]
    pub deleted_at: Option<DateTime<Utc>>,
    /// `None` is the Default folder.
    #[serde(default)]
    pub folder: Option<FolderId>,
    #[serde(default)]
    pub status: WorkspaceStatus,
    #[serde(default)]
    pub notes: String,
    /// A pointer, not a state: the frontend decides whether it counts, so
    /// soft-deleting or moving the blocker unblocks without touching this.
    /// Clear it only where the id stops existing.
    #[serde(default)]
    pub blocked_by: Option<WorkspaceId>,
    /// Oldest first.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceStatus {
    #[default]
    Ready,
    /// A draft waiting in the provisioning queue, as opposed to being built.
    Queued,
    Creating,
    CreationFailed {
        error: String,
    },
}

/// Recorded, never displayed or acted on.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Ui,
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
    pub kind: String,
    pub message: String,
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
    /// Includes the branch's own PR, which is only special in being added
    /// automatically.
    #[serde(default)]
    pub prs: Vec<TrackedPr>,
    /// Detached numbers, so branch discovery doesn't re-add them next tick.
    #[serde(default)]
    pub dismissed: Vec<u32>,
    /// Teardown deletes only branches Tethys created. Defaults to `true`:
    /// older state predates checking out existing branches.
    #[serde(default = "default_created_branch")]
    pub created_branch: bool,
}

fn default_created_branch() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedPr {
    pub number: u32,
    pub tracked_at: DateTime<Utc>,
    #[serde(default)]
    pub status: Option<GithubPrStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionMeta {
    /// Also the tmux session name.
    pub id: SessionId,
    pub cwd: PathBuf,
    /// What the CLI resumes by. Claude rotates it on compaction.
    #[serde(alias = "claude_session_id")]
    pub agent_session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    #[serde(default)]
    pub runtime_state: Option<SessionRuntimeState>,
    #[serde(default)]
    pub notification_type: Option<String>,
    /// Reset on the next `runtime_state` transition, so the dot re-lights.
    #[serde(default)]
    pub turn_acknowledged: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionRuntimeState {
    #[default]
    Dormant,
    Working,
    Idle,
    /// The prompt or a permission dialog is waiting on the user.
    WaitingInput,
}

impl Workspace {
    pub fn draft(
        id: WorkspaceId,
        branch: String,
        agent: Agent,
        agent_binary: Option<String>,
        origin: Origin,
        folder: Option<FolderId>,
    ) -> Self {
        Self {
            id,
            branch,
            created_at: Utc::now(),
            repo_links: Vec::new(),
            session: None,
            agent,
            agent_binary,
            origin,
            deleted_at: None,
            folder,
            status: WorkspaceStatus::Creating,
            notes: String::new(),
            blocked_by: None,
            artifacts: Vec::new(),
        }
    }

    /// `None` for drafts and failed creations, which have no repo links.
    pub fn root(&self) -> Option<&Path> {
        self.repo_links
            .first()
            .and_then(|l| l.worktree_path.parent())
    }

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

    /// A late hook from a session killed by `switch_agent` must not land on
    /// its replacement.
    pub fn session_mut(&mut self, session_id: &str) -> Option<&mut AgentSessionMeta> {
        self.session.as_mut().filter(|m| m.id == session_id)
    }

    /// An existing session keeps its cwd so its conversation still resumes.
    pub fn session_cwd(&self) -> Option<PathBuf> {
        if let Some(session) = &self.session {
            return Some(session.cwd.clone());
        }
        self.root_buf()
    }
}

impl RepoLink {
    pub fn tracked(&self, number: u32) -> Option<&TrackedPr> {
        self.prs.iter().find(|p| p.number == number)
    }

    pub fn tracked_mut(&mut self, number: u32) -> Option<&mut TrackedPr> {
        self.prs.iter_mut().find(|p| p.number == number)
    }

    /// Also un-dismisses: asking for a PR outranks having detached it.
    pub fn track(&mut self, number: u32, status: Option<GithubPrStatus>) {
        self.dismissed.retain(|n| *n != number);
        match self.tracked_mut(number) {
            // A failed fetch mustn't blank a good chip.
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

    /// Returns whether it was tracked.
    pub fn untrack(&mut self, number: u32) -> bool {
        let had = self.tracked(number).is_some();
        self.prs.retain(|p| p.number != number);
        if !self.dismissed.contains(&number) {
            self.dismissed.push(number);
        }
        had
    }

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

    /// The hop limit lets a hand-edited `state.json` that already holds a
    /// cycle terminate the walk.
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
        // Already cyclic.
        true
    }

    /// A missing workspace differs from everything, erring towards refusing
    /// a blocker link.
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

    pub fn empty_folder(&mut self, folder_id: &str) {
        for ws in &mut self.workspaces {
            if ws.folder.as_deref() == Some(folder_id) {
                ws.folder = None;
            }
        }
    }

    /// Returns how many moved.
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
            session: None,
            agent: Default::default(),
            agent_binary: None,
            origin: Origin::Ui,
            deleted_at: None,
            folder: None,
            status: WorkspaceStatus::Ready,
            notes: String::new(),
            blocked_by: None,
            artifacts: Vec::new(),
        }
    }

    fn session(id: &str, cwd: &str) -> AgentSessionMeta {
        AgentSessionMeta {
            id: id.into(),
            cwd: PathBuf::from(cwd),
            agent_session_id: None,
            transcript_path: None,
            runtime_state: None,
            notification_type: None,
            turn_acknowledged: false,
        }
    }

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

    #[test]
    fn a_workspace_with_no_repo_links_has_no_root() {
        let mut ws = workspace_with_links(&[]);
        ws.status = WorkspaceStatus::Creating;
        assert_eq!(ws.root(), None);
        assert_eq!(ws.root_buf(), None);
    }

    #[test]
    fn session_mut_answers_only_for_the_current_session() {
        let mut ws = workspace_with_links(&["/wt/ws-1/frontend"]);
        assert!(ws.session_mut("sess-1").is_none());
        ws.session = Some(session("sess-1", "/wt/ws-1"));
        assert!(ws.session_mut("sess-1").is_some());
        assert!(ws.session_mut("sess-2").is_none());
    }

    #[test]
    fn a_fresh_session_runs_in_the_workspace_root() {
        let one = workspace_with_links(&["/wt/ws-1/frontend"]);
        assert_eq!(one.session_cwd(), Some(PathBuf::from("/wt/ws-1")));
        let two = workspace_with_links(&["/wt/ws-1/frontend", "/wt/ws-1/backend"]);
        assert_eq!(two.session_cwd(), Some(PathBuf::from("/wt/ws-1")));
        assert_eq!(workspace_with_links(&[]).session_cwd(), None);
    }

    #[test]
    fn an_existing_session_keeps_its_cwd() {
        let mut ws = workspace_with_links(&["/wt/ws-1/frontend", "/wt/ws-1/backend"]);
        ws.session = Some(session("sess-1", "/wt/ws-1/frontend"));
        assert_eq!(ws.session_cwd(), Some(PathBuf::from("/wt/ws-1/frontend")));
    }

    #[test]
    fn pre_github_state_json_round_trips() {
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
        assert!(ws.repo_links[0].prs.is_empty());
        assert!(ws.repo_links[0].dismissed.is_empty());
        assert!(ws.agent_binary.is_none());
        assert!(ws.deleted_at.is_none());
        assert!(ws.folder.is_none());
        assert!(parsed.system_errors.is_empty());
    }

    #[test]
    fn pre_turn_state_session_round_trips() {
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "session": {
                        "id": "sess-1",
                        "cwd": "/tmp/wt/abc-123/frontend",
                        "claude_session_id": null,
                        "transcript_path": null
                    }
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        let session = parsed.workspaces[0].session.as_ref().expect("session");
        assert_eq!(session.id, "sess-1");
        assert!(session.runtime_state.is_none());
        assert!(session.notification_type.is_none());
        assert!(!session.turn_acknowledged);
    }

    #[test]
    fn pre_agent_state_loads_as_claude() {
        let raw = r#"{
            "workspaces": [
                {
                    "id": "abc-123",
                    "branch": "feat/foo",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "claude_binary": "claude-hipaa",
                    "session": {
                        "id": "sess-1",
                        "cwd": "/tmp/wt/abc-123",
                        "claude_session_id": "csid-1",
                        "transcript_path": null
                    }
                }
            ]
        }"#;
        let parsed: AppState = serde_json::from_str(raw).expect("must deserialize");
        let ws = &parsed.workspaces[0];
        assert_eq!(ws.agent, Agent::Claude);
        assert_eq!(ws.agent_binary.as_deref(), Some("claude-hipaa"));
        assert_eq!(
            ws.session.as_ref().unwrap().agent_session_id.as_deref(),
            Some("csid-1")
        );
    }

    #[test]
    fn pre_status_state_defaults_to_ready() {
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
                        Agent::Claude,
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
        // a <- b
        let state = blocking_state(&[("a", None), ("b", Some("a")), ("c", None)]);
        assert!(!state.blocker_would_cycle("c", "b"));
    }

    #[test]
    fn a_blocker_downstream_of_the_target_would_cycle() {
        // a <- b <- c
        let state = blocking_state(&[("a", None), ("b", Some("a")), ("c", Some("b"))]);
        assert!(state.blocker_would_cycle("a", "c"));
        assert!(state.blocker_would_cycle("a", "b"));
    }

    #[test]
    fn a_preexisting_cycle_does_not_hang_the_walk() {
        let state = blocking_state(&[("a", Some("b")), ("b", Some("a")), ("c", None)]);
        assert!(state.blocker_would_cycle("c", "a"));
    }

    #[test]
    fn clearing_links_drops_every_dependent() {
        let mut state = blocking_state(&[("a", None), ("b", Some("a")), ("c", Some("a"))]);
        state.clear_links_to("a");
        assert_eq!(state.workspaces[1].blocked_by, None);
        assert_eq!(state.workspaces[2].blocked_by, None);
    }

    #[test]
    fn pre_origin_state_defaults_to_the_ui() {
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

    #[test]
    fn tracking_a_dismissed_number_un_dismisses_it() {
        let mut l = link();
        l.untrack(7);
        l.track(7, Some(pr_status(7)));
        assert!(l.dismissed.is_empty());
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
            review_requested: false,
            unresolved_threads: 0,
            head_branch: None,
            head_sha: String::new(),
            stack: None,
            merge_queue: None,
            fetched_at: Utc::now(),
            last_error: None,
        }
    }

    fn folder_state(members: &[(&str, Option<&str>)]) -> AppState {
        AppState {
            workspaces: members
                .iter()
                .map(|(id, folder)| {
                    Workspace::draft(
                        (*id).into(),
                        format!("branch/{id}"),
                        Agent::Claude,
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

    #[test]
    fn default_folder_counts_as_a_folder_for_blocking() {
        let s = folder_state(&[("a", None), ("b", None), ("c", Some("f1"))]);
        assert!(!s.folders_differ("a", "b"));
        assert!(s.folders_differ("a", "c"));
    }

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
