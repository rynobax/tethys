//! Shared by the attach dialog and the `link_pr` MCP tool.

use crate::error::{AppError, AppResult};
use crate::github::poller::fetch_pr_status;
use crate::github::{parse_pr_reference, resolve_attach_target, GithubPrStatus, GithubSlug};
use crate::registry::RegistryLoad;
use crate::state::{Workspace, WorkspaceId};
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct Attached {
    pub repo_key: String,
    /// The PR's head is the workspace's own branch.
    pub is_branch_pr: bool,
    pub status: GithubPrStatus,
}

/// Fetches before recording so a wrong number fails loudly instead of parking
/// an empty chip.
pub async fn attach(
    store: &Store,
    registry: &RegistryLoad,
    workspace_id: &WorkspaceId,
    repo_key: Option<&str>,
    reference: &str,
) -> AppResult<Attached> {
    let pr = parse_pr_reference(reference).ok_or_else(|| {
        AppError::Other(format!(
            "couldn't read a PR number from \"{}\" — paste a PR URL or a number",
            reference.trim()
        ))
    })?;
    let reg = registry.require()?;

    let repo_keys: Vec<String> = store
        .read(|s| {
            s.find_workspace(workspace_id)
                .map(|w| w.repo_links.iter().map(|r| r.repo_key.clone()).collect())
        })
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(workspace_id.clone()))?;
    let mut candidates: Vec<(String, GithubSlug)> = Vec::new();
    for key in repo_keys {
        if let Some(slug) = reg.find_repo(&key).and_then(|r| r.github_slug.clone()) {
            candidates.push((key, slug));
        }
    }

    let (repo_key, slug) = resolve_attach_target(&candidates, repo_key, &pr)
        .map_err(|e| AppError::Other(e.to_string()))?;

    let status = fetch_pr_status(&slug, pr.number)
        .await
        .map_err(|e| {
            AppError::Other(format!(
                "couldn't fetch PR #{} from {}/{}: {e}",
                pr.number, slug.owner, slug.name
            ))
        })?
        .ok_or_else(|| {
            AppError::Other(format!(
                "{}/{} has no PR #{}",
                slug.owner, slug.name, pr.number
            ))
        })?;

    let stored = status.clone();
    let key = repo_key.clone();
    let is_branch_pr = store
        .update_workspace(workspace_id, move |ws| record(ws, &key, stored))
        .await?;

    Ok(Attached {
        repo_key,
        is_branch_pr,
        status,
    })
}

/// Returns whether it's the branch PR. Re-attaching is a refresh, not an error.
fn record(ws: &mut Workspace, repo_key: &str, status: GithubPrStatus) -> AppResult<bool> {
    let number = status.pr_number;
    let is_branch_pr = status.head_branch.as_deref() == Some(ws.branch.as_str());
    let link = ws
        .link_mut(repo_key)
        .ok_or_else(|| AppError::Other(format!("workspace has no worktree for {repo_key}")))?;
    link.track(number, Some(status));
    Ok(is_branch_pr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::status::ChecksRollup;
    use crate::github::test_support;

    fn workspace() -> Workspace {
        test_support::workspace("ws-1", "feat/thing", "api")
    }

    fn status(number: u32, head_branch: &str) -> GithubPrStatus {
        GithubPrStatus {
            head_branch: Some(head_branch.into()),
            ..test_support::status(number)
        }
    }

    #[test]
    fn a_pr_on_the_workspace_branch_reports_as_the_branch_pr() {
        let mut ws = workspace();
        assert!(record(&mut ws, "api", status(7, "feat/thing")).unwrap());
        let link = ws.link("api").unwrap();
        assert_eq!(link.prs.len(), 1);
        assert_eq!(link.prs[0].number, 7);
    }

    #[test]
    fn a_pr_on_any_other_branch_is_tracked_the_same_way() {
        let mut ws = workspace();
        assert!(!record(&mut ws, "api", status(8, "feat/stacked")).unwrap());
        let link = ws.link("api").unwrap();
        assert_eq!(link.prs.len(), 1);
        assert_eq!(link.prs[0].number, 8);
    }

    #[test]
    fn re_linking_refreshes_rather_than_duplicating() {
        for branch in ["feat/thing", "feat/stacked"] {
            let mut ws = workspace();
            record(&mut ws, "api", status(7, branch)).unwrap();
            let mut newer = status(7, branch);
            newer.checks = ChecksRollup::Failure;
            record(&mut ws, "api", newer).unwrap();
            let link = ws.link("api").unwrap();
            assert_eq!(link.prs.len(), 1, "{branch}");
            assert_eq!(
                link.prs[0].status.as_ref().unwrap().checks,
                ChecksRollup::Failure,
                "{branch}",
            );
        }
    }

    #[test]
    fn attaching_a_detached_pr_un_dismisses_it() {
        let mut ws = workspace();
        record(&mut ws, "api", status(7, "feat/thing")).unwrap();
        ws.link_mut("api").unwrap().untrack(7);
        record(&mut ws, "api", status(7, "feat/thing")).unwrap();
        let link = ws.link("api").unwrap();
        assert_eq!(link.prs.len(), 1);
        assert!(link.dismissed.is_empty());
    }

    #[test]
    fn a_status_with_no_head_branch_is_not_the_branch_pr() {
        let mut ws = workspace();
        let mut s = status(9, "ignored");
        s.head_branch = None;
        assert!(!record(&mut ws, "api", s).unwrap());
    }

    #[test]
    fn a_repo_the_workspace_does_not_have_is_an_error() {
        let mut ws = workspace();
        let err = record(&mut ws, "web", status(1, "feat/thing")).unwrap_err();
        assert!(err.to_string().contains("no worktree for web"), "{err}");
    }
}
