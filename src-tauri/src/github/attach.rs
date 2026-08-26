//! Pointing a workspace at a pull request.
//!
//! Two callers arrive here: the user pasting a reference into the attach
//! dialog, and an agent calling `link_pr` over the MCP socket. They differ only
//! in how the reference reaches the app, so everything past that point —
//! resolving which repo it belongs to, fetching it, and recording it — lives
//! here rather than in the Tauri command.
//!
//! There used to be a third thing here: deciding which of a repo link's two PR
//! slots the reference landed in, since the PR for the workspace's own branch
//! had a slot of its own. It doesn't any more. A link tracks one list, the
//! poller adds the branch's PR to it the same way this does, and both paths
//! land on `RepoLink::track`. Attaching a PR the poller already found is a
//! refresh rather than a duplicate for the same reason it always was — the
//! number is the identity — it just no longer takes a special case to say so.

use crate::error::{AppError, AppResult};
use crate::github::poller::fetch_pr_status;
use crate::github::{parse_pr_reference, resolve_attach_target, GithubPrStatus, GithubSlug};
use crate::registry::RegistryLoad;
use crate::state::{Workspace, WorkspaceId};
use crate::store::Store;

/// Where a reference ended up, and what GitHub said about it.
#[derive(Debug, Clone)]
pub struct Attached {
    pub repo_key: String,
    /// Whether this is the PR for the workspace's own branch — the one the
    /// poller would have found on its own. Read straight off `head_branch`
    /// rather than from where it was stored, because there is only one place
    /// to store it. Reported back to an agent so it can tell "I linked the PR I
    /// just opened" from "I linked somebody else's".
    pub is_branch_pr: bool,
    pub status: GithubPrStatus,
}

/// Resolve `reference` against `workspace_id`'s GitHub-backed repos, fetch the
/// PR, and record it.
///
/// The status is fetched here rather than left to the next poll tick, so a
/// wrong number fails loudly instead of parking an empty chip in the UI.
///
/// `repo_key` is the caller's explicit choice of repo; `None` means infer it,
/// either from the reference's own `owner/repo` or from the workspace having
/// exactly one GitHub-linked repo.
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
    // Only GitHub-backed repos are attachable — the rest have no slug to query.
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

/// Record a fetched status on the repo link, and say whether it turned out to
/// be the PR for the workspace's own branch.
///
/// Split out from [`attach`] because it is all of the state change and none of
/// the I/O: everything above it needs a workspace, a registry and GitHub, and
/// this needs a `Workspace` and a status.
///
/// Attaching something already tracked is a refresh, never an error. It used to
/// be an error for everything except the branch PR, which made re-pasting a
/// number either helpful or a failure depending on which branch the PR happened
/// to be on — a distinction nobody asked for.
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
    use crate::github::status::{ChecksRollup, PrState, ReviewDecision};
    use chrono::Utc;
    use crate::state::{Origin, RepoLink};
    use std::path::PathBuf;

    fn workspace() -> Workspace {
        let mut ws = Workspace::draft("ws-1".into(), "feat/thing".into(), None, Origin::Ui, None);
        ws.repo_links.push(RepoLink {
            repo_key: "api".into(),
            worktree_path: PathBuf::from("/tmp/ws-1/api"),
            setup_script_ran_at: None,
            prs: Vec::new(),
            dismissed: Vec::new(),
            created_branch: true,
        });
        ws
    }

    fn status(number: u32, head_branch: &str) -> GithubPrStatus {
        GithubPrStatus {
            pr_number: number,
            url: format!("https://github.com/me/api/pull/{number}"),
            state: PrState::Open,
            is_draft: false,
            checks: ChecksRollup::None,
            bugbot: ChecksRollup::None,
            has_merge_conflicts: false,
            review_decision: ReviewDecision::None,
            unresolved_threads: 0,
            head_branch: Some(head_branch.into()),
            stack: None,
            head_sha: "sha".into(),
            fetched_at: Utc::now(),
            last_error: None,
        }
    }

    /// The agent-facing half of the point: an agent that opens the PR for the
    /// branch it is working on gets there before the poller does, and is told
    /// that's what it linked.
    #[test]
    fn a_pr_on_the_workspace_branch_reports_as_the_branch_pr() {
        let mut ws = workspace();
        assert!(record(&mut ws, "api", status(7, "feat/thing")).unwrap());
        let link = ws.link("api").unwrap();
        assert_eq!(link.prs.len(), 1);
        assert_eq!(link.prs[0].number, 7);
    }

    /// Same list, same code path — the only difference is what it reports.
    #[test]
    fn a_pr_on_any_other_branch_is_tracked_the_same_way() {
        let mut ws = workspace();
        assert!(!record(&mut ws, "api", status(8, "feat/stacked")).unwrap());
        let link = ws.link("api").unwrap();
        assert_eq!(link.prs.len(), 1);
        assert_eq!(link.prs[0].number, 8);
    }

    /// Re-linking is a refresh whichever branch the PR is on. This used to hold
    /// only for the branch PR, and error for everything else.
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

    /// Asking for a PR by number outranks having detached it, or the only way
    /// back from a mis-click would be editing `state.json`.
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

    /// A PR whose head branch GitHub didn't report can't be claimed as the
    /// workspace's own — a workspace branch is always a `Some`.
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
