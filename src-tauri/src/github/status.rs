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
    /// No checks configured for the head commit.
    None,
    Pending,
    Success,
    Failure,
    /// GitHub returns NEUTRAL/SKIPPED rollups — treat them as "not failing".
    Neutral,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// No reviews yet, or not applicable (non-Open PR).
    #[default]
    None,
    Approved,
    ChangesRequested,
    /// Required reviewers haven't weighed in yet.
    ReviewRequired,
}

/// Where a PR sits in its repo's merge queue, mirroring GitHub's
/// `MergeQueueEntryState`. Only a repo with a queue configured ever produces
/// one, so `None` covers both "no queue here" and "not queued yet" — the chip
/// draws nothing in either case, which is what it did before queues existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeQueueState {
    /// In line, nothing running for it yet.
    Queued,
    /// At the front: GitHub is building and testing the merge branch.
    AwaitingChecks,
    /// Checks passed; it merges as soon as the queue reaches it.
    Mergeable,
    /// The queue is about to eject it — its merge branch failed or conflicts.
    Unmergeable,
    /// Held by a lock on the base branch (a deploy freeze, usually).
    Locked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubPrStatus {
    pub pr_number: u32,
    pub url: String,
    pub state: PrState,
    pub is_draft: bool,
    pub checks: ChecksRollup,
    /// Status of the Cursor Bugbot check, if present. Tracked separately from
    /// the rest of the CI rollup so the UI can show it as its own indicator.
    #[serde(default = "default_bugbot")]
    pub bugbot: ChecksRollup,
    /// True when GitHub reports the PR as having merge conflicts with its
    /// base branch (`mergeable: CONFLICTING`). The UI surfaces this through
    /// the same CI indicator since you can't merge regardless of CI state.
    #[serde(default)]
    pub has_merge_conflicts: bool,
    #[serde(default)]
    pub review_decision: ReviewDecision,
    pub unresolved_threads: u32,
    /// The PR's head branch. What tells two PRs on the same repo link apart,
    /// and what `attach` reads to report whether a linked PR is the one for the
    /// workspace's own branch. `None` for statuses persisted before this field
    /// existed.
    #[serde(default)]
    pub head_branch: Option<String>,
    /// Where the PR sits in a `gh stack`, if it's in one. `None` both for a
    /// lone PR and for PRs merely based on each other by hand — GitHub reports
    /// this only once the stack is a real object on its side.
    #[serde(default)]
    pub stack: Option<PrStack>,
    /// Set only while the PR is sitting in a merge queue. This is what tells
    /// "queued to merge" apart from "nobody has pressed the button yet": both
    /// are `PrState::Open`, and GitHub reports no other difference between
    /// them on the PR itself.
    #[serde(default)]
    pub merge_queue: Option<MergeQueueState>,
    pub head_sha: String,
    pub fetched_at: DateTime<Utc>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// A PR's membership in a GitHub stack, flattened from GraphQL's `stack` +
/// `stackEntry`. A PR is in at most one stack, and a position means nothing
/// without the stack it indexes into, so the two travel together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrStack {
    /// Identifies the stack within its repository — stack-mates share it. Not
    /// a PR number, though GitHub draws both from one sequence.
    pub number: u32,
    /// Total PRs in the stack, including any this workspace doesn't track.
    pub size: u32,
    /// This PR's slot, where 1 is closest to the stack's base branch.
    pub position: u32,
}

fn default_bugbot() -> ChecksRollup {
    ChecksRollup::None
}
