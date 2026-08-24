//! Pointing a workspace at a pull request.
//!
//! Two callers arrive here: the user pasting a reference into the attach
//! dialog, and an agent calling `link_pr` over the MCP socket. They differ only
//! in how the reference reaches the app, so everything past that point —
//! resolving which repo it belongs to, fetching it, and deciding *which slot on
//! the repo link it lands in* — lives here rather than in the Tauri command.
//!
//! That last decision is the reason this isn't just a `push`. A repo link has
//! two places a PR can sit: `github`, the PR for the workspace's own branch,
//! which the poller discovers on its own; and `attached_prs`, everything else.
//! An agent that just opened a PR for the branch it is working on would
//! otherwise land in `attached_prs`, and then the poller would find the same PR
//! by branch a tick later and draw it twice. Routing on `head_branch` makes the
//! two paths agree, and has the side benefit that the chip appears at once
//! instead of at the next poll.

use chrono::Utc;

use crate::error::{AppError, AppResult};
use crate::github::poller::fetch_pr_status;
use crate::github::{parse_pr_reference, resolve_attach_target, GithubPrStatus, GithubSlug};
use crate::registry::RegistryLoad;
use crate::state::{AttachedPr, Workspace, WorkspaceId};
use crate::store::Store;

/// Which of a repo link's two PR slots the reference landed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachSlot {
    /// The PR for the workspace's own branch — the slot the poller maintains.
    BranchPr,
    /// A PR opened from this worktree on some other branch.
    Attached,
}

/// Where a reference ended up, and what GitHub said about it.
#[derive(Debug, Clone)]
pub struct Attached {
    pub repo_key: String,
    pub slot: AttachSlot,
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
    let slot = store
        .update_workspace(workspace_id, move |ws| record(ws, &key, stored))
        .await?;

    Ok(Attached {
        repo_key,
        slot,
        status,
    })
}

/// Write a fetched status into whichever of the repo link's two PR slots it
/// belongs in, and say which one that was.
///
/// Split out from [`attach`] because it is the whole of the decision and none
/// of the I/O: everything above it needs a workspace, a registry and GitHub,
/// and this needs a `Workspace` and a status.
fn record(ws: &mut Workspace, repo_key: &str, status: GithubPrStatus) -> AppResult<AttachSlot> {
    let number = status.pr_number;
    let is_branch_pr = status.head_branch.as_deref() == Some(ws.branch.as_str());
    let link = ws
        .link_mut(repo_key)
        .ok_or_else(|| AppError::Other(format!("workspace has no worktree for {repo_key}")))?;

    if is_branch_pr {
        // Idempotent on purpose: re-pointing at the branch PR is a refresh, not
        // a mistake worth an error. Any copy that reached `attached_prs` before
        // the poller caught up goes away here, so the two paths can't both draw
        // the same PR.
        link.attached_prs.retain(|a| a.number != number);
        link.github = Some(status);
        return Ok(AttachSlot::BranchPr);
    }

    if link.attached_prs.iter().any(|a| a.number == number) {
        return Err(AppError::Other(format!(
            "PR #{number} is already attached to {repo_key}"
        )));
    }
    link.attached_prs.push(AttachedPr {
        number,
        attached_at: Utc::now(),
        status: Some(status),
    });
    Ok(AttachSlot::Attached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::status::{ChecksRollup, PrState, ReviewDecision};
    use crate::state::{Origin, RepoLink};
    use std::path::PathBuf;

    fn workspace() -> Workspace {
        let mut ws = Workspace::draft("ws-1".into(), "feat/thing".into(), None, Origin::Ui, None);
        ws.repo_links.push(RepoLink {
            repo_key: "api".into(),
            worktree_path: PathBuf::from("/tmp/ws-1/api"),
            setup_script_ran_at: None,
            github: None,
            attached_prs: Vec::new(),
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
            head_sha: "sha".into(),
            fetched_at: Utc::now(),
            last_error: None,
        }
    }

    /// An agent that opens the PR for the branch it is working on and links it
    /// immediately gets there before the poller does. It has to land in the
    /// branch slot anyway, or the poller finds the same PR a tick later and the
    /// workspace draws it twice.
    #[test]
    fn a_pr_on_the_workspace_branch_fills_the_branch_slot() {
        let mut ws = workspace();
        let slot = record(&mut ws, "api", status(7, "feat/thing")).unwrap();
        assert_eq!(slot, AttachSlot::BranchPr);
        let link = ws.link("api").unwrap();
        assert_eq!(link.github.as_ref().unwrap().pr_number, 7);
        assert!(link.attached_prs.is_empty());
    }

    #[test]
    fn a_pr_on_any_other_branch_is_attached() {
        let mut ws = workspace();
        let slot = record(&mut ws, "api", status(8, "feat/stacked")).unwrap();
        assert_eq!(slot, AttachSlot::Attached);
        let link = ws.link("api").unwrap();
        assert!(link.github.is_none());
        assert_eq!(link.attached_prs.len(), 1);
        assert_eq!(link.attached_prs[0].number, 8);
    }

    /// Re-linking the branch PR is a refresh, not a mistake — the second call
    /// has to overwrite rather than error or duplicate.
    #[test]
    fn re_linking_the_branch_pr_refreshes_it() {
        let mut ws = workspace();
        record(&mut ws, "api", status(7, "feat/thing")).unwrap();
        let mut newer = status(7, "feat/thing");
        newer.checks = ChecksRollup::Failure;
        assert_eq!(record(&mut ws, "api", newer).unwrap(), AttachSlot::BranchPr);
        let link = ws.link("api").unwrap();
        assert_eq!(link.github.as_ref().unwrap().checks, ChecksRollup::Failure);
        assert!(link.attached_prs.is_empty());
    }

    /// State written before this routing existed can hold the branch PR in
    /// `attached_prs`. Linking it again has to collapse the two, not add a
    /// third rendering of the same PR.
    #[test]
    fn a_stale_attached_copy_of_the_branch_pr_is_swept_up() {
        let mut ws = workspace();
        ws.link_mut("api").unwrap().attached_prs.push(AttachedPr {
            number: 7,
            attached_at: Utc::now(),
            status: Some(status(7, "feat/thing")),
        });
        record(&mut ws, "api", status(7, "feat/thing")).unwrap();
        let link = ws.link("api").unwrap();
        assert_eq!(link.github.as_ref().unwrap().pr_number, 7);
        assert!(link.attached_prs.is_empty());
    }

    #[test]
    fn attaching_the_same_extra_pr_twice_is_rejected() {
        let mut ws = workspace();
        record(&mut ws, "api", status(8, "feat/stacked")).unwrap();
        let err = record(&mut ws, "api", status(8, "feat/stacked")).unwrap_err();
        assert!(err.to_string().contains("already attached"), "{err}");
        assert_eq!(ws.link("api").unwrap().attached_prs.len(), 1);
    }

    /// A PR whose head branch GitHub didn't report can't be claimed as the
    /// workspace's own — a workspace branch is always a `Some`.
    #[test]
    fn a_status_with_no_head_branch_is_attached() {
        let mut ws = workspace();
        let mut s = status(9, "ignored");
        s.head_branch = None;
        assert_eq!(record(&mut ws, "api", s).unwrap(), AttachSlot::Attached);
    }

    #[test]
    fn a_repo_the_workspace_does_not_have_is_an_error() {
        let mut ws = workspace();
        let err = record(&mut ws, "web", status(1, "feat/thing")).unwrap_err();
        assert!(err.to_string().contains("no worktree for web"), "{err}");
    }
}
