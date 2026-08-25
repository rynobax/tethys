//! Creating a workspace on an agent's behalf.
//!
//! A **handoff** is the same workspace the user would have made in the UI,
//! asked for from inside a running session instead. The differences are all at
//! the edges:
//!
//! - It returns the moment the draft is in state, long before the worktrees
//!   exist. The calling agent is told the handoff was *accepted*, never how it
//!   went — provisioning takes minutes, and an agent that waited that long for
//!   an answer it can't act on is worse than one that moved on.
//! - It always starts exactly one session, at the workspace root, with the
//!   Brief as its first message. A handoff with nobody picking the work up is
//!   just an expensive empty workspace.
//! - The branch is auto-suffixed rather than refused when taken. "Pick another
//!   name" is advice a non-interactive caller can't take.
//! - It inherits the calling workspace's `claude_binary`, and the agent can't
//!   ask for a different one. Handing work from a `claude-hipaa` workspace to a
//!   plain `claude` one would move it across that boundary by accident.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::inprogress::InProgressWorkspaces;
use crate::job::JobTx;
use crate::mcp::{CreateWorkspace, McpLaunch};
use crate::paths::Paths;
use crate::provision::{provision_workspace, WorkspaceProvision};
use crate::provision_queue::ProvisionQueue;
use crate::registry::{self, RegistryLoad, Repo};
use crate::sessions::{self, SessionSupervisor, StartSession};
use crate::state::{Origin, Workspace, WorkspaceId};
use crate::store::Store;

/// How many `-2`, `-3`… suffixes to try before giving up on a branch name.
const MAX_BRANCH_SUFFIX: u32 = 50;

/// What the calling agent is told: a workspace exists, under this branch.
pub struct Accepted {
    pub workspace_id: WorkspaceId,
    pub branch: String,
}

/// Everything a handoff needs to reach the same machinery the UI uses.
pub struct Handoff {
    store: Arc<Store>,
    registry: Arc<RegistryLoad>,
    paths: Paths,
    in_progress: InProgressWorkspaces,
    /// Shared with the UI path, so a handoff queues behind a workspace the
    /// user asked for by hand — and vice versa.
    queue: ProvisionQueue,
    supervisor: Arc<SessionSupervisor>,
    /// Empty when tmux didn't resolve at boot, in which case the workspace
    /// still gets provisioned and only its session is skipped.
    tmux_bin: PathBuf,
    claude_bin: PathBuf,
    /// The config handed to the new workspace's session, so it can hand off in
    /// turn. Uniform on purpose: whether an agent can hand off shouldn't depend
    /// on how its workspace came to exist.
    mcp: Option<McpLaunch>,
}

impl Handoff {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<Store>,
        registry: Arc<RegistryLoad>,
        paths: Paths,
        in_progress: InProgressWorkspaces,
        queue: ProvisionQueue,
        supervisor: Arc<SessionSupervisor>,
        tmux_bin: PathBuf,
        claude_bin: PathBuf,
        mcp: Option<McpLaunch>,
    ) -> Self {
        Self {
            store,
            registry,
            paths,
            in_progress,
            queue,
            supervisor,
            tmux_bin,
            claude_bin,
            mcp,
        }
    }

    /// Validate, reserve a branch, insert the draft, and return.
    ///
    /// Provisioning and the session spawn continue in a detached task, so
    /// everything that can be refused has to be refused *here* — once this
    /// returns `Ok`, the only remaining channel for a failure is the
    /// `CreationFailed` row in Tethys.
    pub async fn accept(self: &Arc<Self>, req: CreateWorkspace) -> AppResult<Accepted> {
        let reg = self.registry.require()?;

        let brief = req.brief.trim().to_string();
        if brief.is_empty() {
            return Err(AppError::Other(
                "a brief is required — it's the only thing the new session gets".into(),
            ));
        }

        if req.repos.is_empty() {
            return Err(AppError::Other(
                "name at least one repo for the new workspace".into(),
            ));
        }
        let selected: Vec<Repo> = req
            .repos
            .iter()
            .map(|k| {
                reg.find_repo(k).cloned().ok_or_else(|| {
                    let known: Vec<&str> = reg.repos.iter().map(|r| r.key.as_str()).collect();
                    AppError::Other(format!(
                        "unknown repo key: {k}. Known repos: {}",
                        known.join(", ")
                    ))
                })
            })
            .collect::<AppResult<Vec<_>>>()?;

        let requested = validate_branch(&req.branch)?;

        // The caller's workspace is the origin *and* the source of the binary
        // the new one runs under. A caller that isn't in state is a stale
        // session from a workspace that's since been forgotten.
        let caller = self
            .store
            .read(|s| s.find_workspace(&req.from_workspace).cloned())
            .await
            .ok_or_else(|| {
                AppError::Other(format!(
                    "the calling workspace ({}) is no longer in Tethys",
                    req.from_workspace
                ))
            })?;

        let (branch, workspace_dir) = self.reserve_branch(reg, &requested)?;

        let id = uuid::Uuid::new_v4().to_string();
        let draft = Workspace::draft(
            id.clone(),
            branch.clone(),
            caller.claude_binary.clone(),
            Origin::Handoff {
                from_workspace: caller.id.clone(),
                from_session: req.from_session.clone(),
            },
            // Lands beside the session that asked for it. Agents get no say in
            // the folder for the same reason they get no say in the binary:
            // work shouldn't drift out of where the user put it.
            caller.folder.clone(),
        );
        // Setting the link in the same mutation as the insert is what makes a
        // `Creating` draft a legal blocker: the caller points at the new
        // workspace minutes before it finishes provisioning. Overwrites any
        // blocker already there — fan-in is capped at one, and the most recent
        // declaration is the one to honour.
        let blocks_caller = req.blocks_caller;
        let caller_id = caller.id.clone();
        self.store
            .mutate(|s| {
                s.workspaces.insert(0, draft.clone());
                if blocks_caller {
                    if let Some(ws) = s.find_workspace_mut(&caller_id) {
                        ws.blocked_by = Some(draft.id.clone());
                    }
                }
                Ok(())
            })
            .await?;
        self.store.notify_changed(&id);
        if blocks_caller {
            self.store.notify_changed(&caller.id);
        }

        info!(
            workspace = %id,
            branch = %branch,
            from_workspace = %caller.id,
            repos = selected.len(),
            "handoff accepted"
        );

        let this = self.clone();
        let task_branch = branch.clone();
        let task_id = id.clone();
        tauri::async_runtime::spawn(async move {
            this.provision_and_start(task_id, task_branch, workspace_dir, selected, brief)
                .await;
        });

        Ok(Accepted {
            workspace_id: id,
            branch,
        })
    }

    /// The detached half: provision the worktrees, then start the one session
    /// that picks the work up. Nothing here can be reported to the caller, so
    /// everything lands in the log and in the workspace's own status.
    async fn provision_and_start(
        &self,
        workspace_id: String,
        branch: String,
        workspace_dir: String,
        repos: Vec<Repo>,
        brief: String,
    ) {
        let Ok(reg) = self.registry.require() else {
            return;
        };

        // No UI channel to stream into — the log and the workspace's own status
        // are the whole story for a handoff.
        let tx = JobTx::silent();
        let provisioned = provision_workspace(WorkspaceProvision {
            workspace_id: &workspace_id,
            branch: &branch,
            workspace_dir: &workspace_dir,
            repos: &repos,
            registry: reg,
            paths: &self.paths,
            store: &self.store,
            in_progress: &self.in_progress,
            queue: &self.queue,
            tx: &tx,
        })
        .await;

        if let Err(e) = provisioned {
            warn!(
                workspace = %workspace_id,
                error = %e,
                "handoff workspace failed to provision; no session started"
            );
            return;
        }

        if self.tmux_bin.as_os_str().is_empty() {
            warn!(
                workspace = %workspace_id,
                "tmux unavailable — handoff workspace provisioned but has no session"
            );
            return;
        }

        match sessions::start_session(StartSession {
            supervisor: &self.supervisor,
            store: &self.store,
            workspace_id: &workspace_id,
            repo_key: None,
            claude_bin: &self.claude_bin,
            tmux_bin: &self.tmux_bin,
            mcp: self.mcp.as_ref(),
            resume_claude_sid: None,
            session_binary: None,
            brief: Some(&brief),
        })
        .await
        {
            Ok(info) => info!(
                workspace = %workspace_id,
                session = %info.id,
                "handoff session started with its brief"
            ),
            Err(e) => warn!(
                workspace = %workspace_id,
                error = %e,
                "handoff workspace is ready but its session failed to start"
            ),
        }
    }

    /// Find a branch name whose workspace directory is free, suffixing `-2`,
    /// `-3`… as needed. The directory is the thing that has to be unique: two
    /// workspaces sharing one would mean deleting either clobbers the other.
    ///
    /// A directory being provisioned right now doesn't exist on disk yet, so
    /// the in-progress set is checked too — otherwise two handoffs landing at
    /// once would both pick the same name. That still leaves the instant
    /// between accepting a handoff and its task registering, which is why the
    /// on-disk collision check inside provisioning stays where it is.
    fn reserve_branch(
        &self,
        reg: &crate::registry::RepoRegistry,
        requested: &str,
    ) -> AppResult<(String, String)> {
        let provisioning = self.in_progress.snapshot();
        for attempt in 1..=MAX_BRANCH_SUFFIX {
            let branch = if attempt == 1 {
                requested.to_string()
            } else {
                format!("{requested}-{attempt}")
            };
            let dir = registry::sanitize_branch_for_dir(&branch);
            if !provisioning.contains(&dir) && !reg.worktree_root.join(&dir).exists() {
                return Ok((branch, dir));
            }
        }
        Err(AppError::Other(format!(
            "no free workspace directory for `{requested}` after {MAX_BRANCH_SUFFIX} attempts"
        )))
    }
}

/// Reject branch names that would confuse git or the filesystem before they
/// reach an argv. The UI path doesn't need this — the user typing a branch is
/// trusted, and finds out immediately. An agent is neither.
fn validate_branch(branch: &str) -> AppResult<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(AppError::Other("a branch name is required".into()));
    }
    // A leading dash reaches `git worktree add` as a flag, not a value.
    if branch.starts_with('-') {
        return Err(AppError::Other(
            "branch name may not start with '-'".into(),
        ));
    }
    if branch.starts_with('/') || branch.ends_with('/') || branch.contains("//") {
        return Err(AppError::Other(
            "branch name may not start, end, or double up on '/'".into(),
        ));
    }
    if branch.contains("..") {
        return Err(AppError::Other("branch name may not contain '..'".into()));
    }
    if let Some(bad) = branch
        .chars()
        .find(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\\"'$`".contains(*c))
    {
        return Err(AppError::Other(format!(
            "branch name may not contain {bad:?}"
        )));
    }
    Ok(branch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_dash_is_refused() {
        // Would arrive at `git worktree add <path> --force` as a flag.
        assert!(validate_branch("--force").is_err());
        assert!(validate_branch("-x").is_err());
    }

    #[test]
    fn ordinary_branch_names_pass() {
        for ok in ["feat/handoff", "fix-123", "ryan/spike_2"] {
            assert_eq!(validate_branch(ok).as_deref().ok(), Some(ok));
        }
    }

    #[test]
    fn whitespace_and_git_metacharacters_are_refused() {
        for bad in [
            "feat/two words",
            "feat/a..b",
            "feat/a~1",
            "feat/a^",
            "feat/a:b",
            "feat/a?b",
            "feat/a*",
            "/leading",
            "trailing/",
            "double//slash",
            "quote'inject",
            "sub$(shell)",
        ] {
            assert!(validate_branch(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_not_refused() {
        assert_eq!(validate_branch("  feat/x  ").unwrap(), "feat/x");
    }
}
