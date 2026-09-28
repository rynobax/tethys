use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksRollup {
    None,
    Pending,
    Success,
    Failure,
    /// GitHub's NEUTRAL/SKIPPED: not failing.
    Neutral,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    #[default]
    None,
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// GitHub's `MergeQueueEntryState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeQueueState {
    Queued,
    AwaitingChecks,
    Mergeable,
    Unmergeable,
    Locked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubPrStatus {
    pub pr_number: u32,
    pub url: String,
    pub state: PrState,
    pub is_draft: bool,
    pub checks: ChecksRollup,
    /// The Cursor Bugbot check, excluded from `checks`.
    #[serde(default = "default_bugbot")]
    pub bugbot: ChecksRollup,
    #[serde(default)]
    pub has_merge_conflicts: bool,
    #[serde(default)]
    pub review_decision: ReviewDecision,
    /// A requested reviewer hasn't submitted yet; GitHub drops the request on
    /// any submission. `REVIEW_REQUIRED` can't distinguish this from "nobody
    /// has been asked".
    #[serde(default)]
    pub review_requested: bool,
    pub unresolved_threads: u32,
    #[serde(default)]
    pub head_branch: Option<String>,
    /// A `gh stack` membership; hand-chained PRs have none.
    #[serde(default)]
    pub stack: Option<PrStack>,
    #[serde(default)]
    pub merge_queue: Option<MergeQueueState>,
    pub head_sha: String,
    pub fetched_at: DateTime<Utc>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// GraphQL's `stack` + `stackEntry`, flattened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrStack {
    /// Per-repository; not a PR number, though drawn from the same sequence.
    pub number: u32,
    /// Includes PRs this workspace doesn't track.
    pub size: u32,
    /// 1 is closest to the base branch.
    pub position: u32,
}

fn default_bugbot() -> ChecksRollup {
    ChecksRollup::None
}
