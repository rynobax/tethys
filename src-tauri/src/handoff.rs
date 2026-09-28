//! A workspace created from inside a running session. The caller hears only
//! that it was accepted, never how provisioning went. The new workspace
//! inherits the caller's binary so work can't leave a `claude-hipaa` workspace
//! by accident.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::agent_bin::AgentBins;
use crate::branch_name;
use crate::error::{AppError, AppResult};
use crate::inprogress::InProgressWorkspaces;
use crate::job::JobTx;
use crate::mcp::{CreateWorkspace, McpLaunch};
use crate::paths::Paths;
use crate::provision::{provision_workspace, WorkspaceProvision};
use crate::provision_queue::ProvisionQueue;
use crate::registry::{RegistryLoad, Repo};
use crate::sessions::{self, OpenSession, SessionSupervisor};
use crate::state::{Origin, Workspace, WorkspaceId};
use crate::store::Store;

pub struct Accepted {
    pub workspace_id: WorkspaceId,
    pub branch: String,
}

pub struct Handoff {
    store: Arc<Store>,
    registry: Arc<RegistryLoad>,
    paths: Paths,
    in_progress: InProgressWorkspaces,
    queue: ProvisionQueue,
    supervisor: Arc<SessionSupervisor>,
    /// Empty when tmux didn't resolve at boot.
    tmux_bin: PathBuf,
    agent_bins: AgentBins,
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
        agent_bins: AgentBins,
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
            agent_bins,
            mcp,
        }
    }

    /// Everything refusable is refused here: after `Ok`, the only failure
    /// channel left is a `CreationFailed` row.
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

        let branch_name::Reserved {
            branch,
            workspace_dir,
        } = branch_name::reserve(&reg.worktree_root, &self.in_progress, &requested)?;

        let id = uuid::Uuid::new_v4().to_string();
        let draft = Workspace::draft(
            id.clone(),
            branch.clone(),
            caller.agent,
            caller.agent_binary.clone(),
            Origin::Handoff {
                from_workspace: caller.id.clone(),
                from_session: req.from_session.clone(),
            },
            caller.folder.clone(),
        );
        // Same mutation as the insert, so the caller never points at a missing
        // row. Overwrites any existing blocker.
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

        match sessions::open_session(OpenSession {
            supervisor: &self.supervisor,
            store: &self.store,
            workspace_id: &workspace_id,
            agent_bins: &self.agent_bins,
            tmux_bin: &self.tmux_bin,
            paths: &self.paths,
            mcp: self.mcp.as_ref(),
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
}

/// Only handoffs validate: a branch the user types is trusted, an agent's
/// isn't.
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
