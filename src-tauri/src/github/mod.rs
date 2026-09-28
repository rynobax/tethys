pub mod attach;
pub mod client;
pub mod poller;
pub mod pr_status;
pub mod pr_ref;
pub mod remote_url;
pub mod status;

pub use attach::attach;
pub use poller::GithubPoller;
pub use pr_ref::{parse_pr_reference, resolve_attach_target};
pub use remote_url::{parse_github_remote, GithubSlug};
pub use status::GithubPrStatus;

#[cfg(test)]
pub(crate) mod test_support {
    use chrono::Utc;

    use crate::agent::Agent;
    use crate::github::status::{ChecksRollup, PrState, ReviewDecision};
    use crate::github::GithubPrStatus;
    use crate::state::{Origin, RepoLink, Workspace};

    pub fn workspace(id: &str, branch: &str, repo_key: &str) -> Workspace {
        let mut ws = Workspace::draft(id.into(), branch.into(), Agent::Claude, None, Origin::Ui, None);
        ws.repo_links.push(RepoLink {
            repo_key: repo_key.into(),
            worktree_path: format!("/tmp/{id}/{repo_key}").into(),
            setup_script_ran_at: None,
            prs: Vec::new(),
            dismissed: Vec::new(),
            created_branch: true,
        });
        ws
    }

    pub fn status(number: u32) -> GithubPrStatus {
        GithubPrStatus {
            pr_number: number,
            url: format!("https://github.com/me/repo/pull/{number}"),
            state: PrState::Open,
            is_draft: false,
            checks: ChecksRollup::Success,
            bugbot: ChecksRollup::None,
            has_merge_conflicts: false,
            review_decision: ReviewDecision::None,
            review_requested: false,
            unresolved_threads: 0,
            head_branch: None,
            stack: None,
            merge_queue: None,
            head_sha: "sha".into(),
            fetched_at: Utc::now(),
            last_error: None,
        }
    }
}
