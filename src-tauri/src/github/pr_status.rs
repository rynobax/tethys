//! The GitHub PR status codec: build the GraphQL query, project the response
//! onto `GithubPrStatus`, and apply the results to `AppState`.
//!
//! Split out of `poller.rs` because these two halves have opposite testing
//! stories. Everything here is a free function over `&Value` / `&mut AppState`
//! — no network, no `AppHandle` — and it carries the 30-odd tests that encode
//! the hard-won rules: bugbot split out of the CI rollup, `mergeable: UNKNOWN`
//! not flashing red, the null-`reviewDecision` fallback for unprotected repos.
//! The polling loop next door is the part that needed seams before it could be
//! tested at all.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{json, Value};

use crate::github::status::{ChecksRollup, GithubPrStatus, PrStack, PrState, ReviewDecision};
use crate::github::GithubSlug;
use crate::state::{AppState, WorkspaceId};

#[derive(Debug, Clone)]
pub struct Target {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub slug: GithubSlug,
    pub kind: TargetKind,
}

/// Which PR of a repo link a target (and its result) refers to: the one for
/// the workspace's own branch, or a specific manually-attached number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    Branch(String),
    Pr(u32),
}

#[derive(Debug, Clone)]
pub struct PollResult {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub kind: TargetKind,
    pub status: Option<GithubPrStatus>,
}

impl PollResult {
    /// Payload for `github:status_changed`. `pr_number` is null for the
    /// branch-derived PR and set for a manually-attached one, which is how the
    /// frontend knows which slot to update.
    pub fn event(&self) -> Value {
        let pr_number = match self.kind {
            TargetKind::Branch(_) => None,
            TargetKind::Pr(n) => Some(n),
        };
        json!({
            "workspace_id": self.workspace_id,
            "repo_key": self.repo_key,
            "pr_number": pr_number,
            "status": self.status,
        })
    }
}

pub fn build_query(targets: &[Target]) -> (String, BTreeMap<String, String>) {
    let mut vars = BTreeMap::new();
    let mut var_decls = Vec::new();
    let mut body = String::new();

    // Shared selection set for a PR node — same shape whether we found it via
    // the branch ref or via the merged-PRs fallback.
    const PR_FIELDS: &str = r#"number
          url
          state
          isDraft
          mergeable
          headRefName
          stack {
            number
            size
          }
          stackEntry {
            position
          }
          reviewDecision
          latestOpinionatedReviews(first: 20) {
            nodes {
              state
              author { login }
            }
          }
          reviewThreads(first: 50) {
            nodes {
              isResolved
              comments(first: 1) {
                nodes { author { login } }
              }
            }
          }
          commits(last: 1) {
            nodes {
              commit {
                oid
                statusCheckRollup {
                  state
                  contexts(first: 100) {
                    nodes {
                      __typename
                      ... on CheckRun {
                        name
                        status
                        conclusion
                      }
                      ... on StatusContext {
                        context
                        state
                      }
                    }
                  }
                }
              }
            }
          }"#;

    for (i, t) in targets.iter().enumerate() {
        let ow = format!("q{i}_owner");
        let nm = format!("q{i}_name");
        vars.insert(ow.clone(), t.slug.owner.clone());
        vars.insert(nm.clone(), t.slug.name.clone());

        match &t.kind {
            TargetKind::Branch(branch) => {
                let br = format!("q{i}_branch");
                let bn = format!("q{i}_branch_name");
                vars.insert(br.clone(), format!("refs/heads/{branch}"));
                vars.insert(bn.clone(), branch.clone());
                var_decls.push(format!(
                    "${ow}: String!, ${nm}: String!, ${br}: String!, ${bn}: String!"
                ));
                // `mergedPrs` is the fallback for when the branch has been
                // deleted post-merge: GitHub nulls the `ref`, but the PR record
                // persists and is queryable by headRefName.
                body.push_str(&format!(
                    r#"q{i}: repository(owner: ${ow}, name: ${nm}) {{
    ref(qualifiedName: ${br}) {{
      associatedPullRequests(first: 1, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
        nodes {{
          {PR_FIELDS}
        }}
      }}
    }}
    mergedPrs: pullRequests(headRefName: ${bn}, states: [MERGED, CLOSED], first: 1, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
      nodes {{
        {PR_FIELDS}
      }}
    }}
  }}
"#
                ));
            }
            // The number is inlined rather than passed as a variable: `gh api
            // graphql -f` only sends strings, and GitHub's `number` argument is
            // an `Int!`. It's a `u32` we parsed ourselves, so there's nothing
            // to inject.
            TargetKind::Pr(number) => {
                var_decls.push(format!("${ow}: String!, ${nm}: String!"));
                body.push_str(&format!(
                    r#"q{i}: repository(owner: ${ow}, name: ${nm}) {{
    pullRequest(number: {number}) {{
      {PR_FIELDS}
    }}
  }}
"#
                ));
            }
        }
    }

    let decls = var_decls.join(", ");
    let query = format!("query({decls}) {{\n  {body}}}\n");
    (query, vars)
}

pub fn parse_response(targets: &[Target], data: &Value) -> Vec<PollResult> {
    let mut out = Vec::with_capacity(targets.len());
    for (i, t) in targets.iter().enumerate() {
        let alias = format!("q{i}");
        let node = data.get(&alias);
        let status = match t.kind {
            TargetKind::Branch(_) => node.and_then(parse_branch_repo_node),
            TargetKind::Pr(_) => node
                .and_then(|repo| repo.get("pullRequest"))
                .and_then(parse_pr_node),
        };
        out.push(PollResult {
            workspace_id: t.workspace_id.clone(),
            repo_key: t.repo_key.clone(),
            kind: t.kind.clone(),
            status,
        });
    }
    out
}

fn parse_branch_repo_node(repo: &Value) -> Option<GithubPrStatus> {
    // Prefer the PR associated with the live branch ref. If the branch was
    // deleted on merge, `ref` will be null — fall back to the most recent
    // merged/closed PR for that branch name.
    let assoc = repo
        .get("ref")
        .and_then(|r| r.get("associatedPullRequests"))
        .and_then(|a| a.get("nodes"))
        .and_then(|n| n.as_array())
        .and_then(|arr| arr.first());
    let merged_fallback = repo
        .get("mergedPrs")
        .and_then(|m| m.get("nodes"))
        .and_then(|n| n.as_array())
        .and_then(|arr| arr.first());
    parse_pr_node(assoc.or(merged_fallback)?)
}

fn parse_pr_node(pr: &Value) -> Option<GithubPrStatus> {
    let number = pr.get("number")?.as_u64()? as u32;
    let url = pr.get("url")?.as_str()?.to_string();
    let state = match pr.get("state")?.as_str()? {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => return None,
    };
    let is_draft = pr.get("isDraft").and_then(|v| v.as_bool()).unwrap_or(false);

    // GitHub's `mergeable` is `MERGEABLE | CONFLICTING | UNKNOWN`. UNKNOWN
    // shows up briefly after a push while GitHub computes the merge — only
    // treat the explicit CONFLICTING signal as a conflict, so we don't flash
    // a false-positive red square during recomputation.
    let has_merge_conflicts = pr
        .get("mergeable")
        .and_then(|v| v.as_str())
        .map(|s| s == "CONFLICTING")
        .unwrap_or(false);

    let review_decision = if state == PrState::Open {
        match pr.get("reviewDecision").and_then(|v| v.as_str()) {
            Some("APPROVED") => ReviewDecision::Approved,
            Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
            Some("REVIEW_REQUIRED") => ReviewDecision::ReviewRequired,
            // Null whenever the base branch doesn't require reviews — GitHub
            // has no verdict to report, but approvals still exist. Without
            // this fallback the review square stays gray forever on every PR
            // in an unprotected repo.
            _ => review_decision_from_reviews(pr),
        }
    } else {
        ReviewDecision::None
    };

    // Walk threads once, splitting unresolved counts between human reviewers
    // and bugbot. The human count drives the review (eye) square; the bugbot
    // count drives the bugbot square — resolving a bugbot finding clears it.
    let (unresolved_threads, bugbot_unresolved) = if state == PrState::Open {
        pr.get("reviewThreads")
            .and_then(|r| r.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|arr| {
                let mut human = 0u32;
                let mut bugbot = 0u32;
                for t in arr {
                    let unresolved = !t
                        .get("isResolved")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !unresolved {
                        continue;
                    }
                    if thread_first_author(t) == Some(BUGBOT_LOGIN) {
                        bugbot += 1;
                    } else {
                        human += 1;
                    }
                }
                (human, bugbot)
            })
            .unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    let commit = pr
        .get("commits")
        .and_then(|c| c.get("nodes"))
        .and_then(|n| n.as_array())
        .and_then(|arr| arr.first())
        .and_then(|node| node.get("commit"));
    let head_sha = commit
        .and_then(|c| c.get("oid"))
        .and_then(|o| o.as_str())
        .unwrap_or("")
        .to_string();
    let rollup = commit.and_then(|c| c.get("statusCheckRollup"));
    let rollup_state = rollup
        .and_then(|r| if r.is_null() { None } else { r.get("state") })
        .and_then(|s| s.as_str())
        .map(rollup_state_from_str);
    let context_nodes = rollup
        .and_then(|r| r.get("contexts"))
        .and_then(|c| c.get("nodes"))
        .and_then(|n| n.as_array());

    // Split bugbot out of the rollup. If we have per-context data, recompute
    // the non-bugbot aggregate so the CI indicator isn't poisoned by bugbot's
    // result. Otherwise fall back to the top-level rollup state.
    let (checks, bugbot_check) = match context_nodes {
        Some(nodes) => {
            let mut non_bugbot = Vec::new();
            let mut bugbot_states = Vec::new();
            for node in nodes {
                let Some(state) = context_state(node) else {
                    continue;
                };
                if context_is_bugbot(node) {
                    bugbot_states.push(state);
                } else {
                    non_bugbot.push(state);
                }
            }
            let checks = aggregate_rollup(non_bugbot.into_iter());
            let bugbot_check = aggregate_rollup(bugbot_states.into_iter());
            (checks, bugbot_check)
        }
        None => (rollup_state.unwrap_or(ChecksRollup::None), ChecksRollup::None),
    };

    // Bugbot's CheckRun conclusion is unreliable — it can complete as
    // SUCCESS or NEUTRAL even when bugbot found bugs. Drive the indicator
    // off unresolved bugbot review threads instead, so resolving a finding
    // (or pushing a fix that GitHub auto-resolves) clears the square.
    let bugbot = if bugbot_unresolved > 0 {
        ChecksRollup::Failure
    } else if matches!(bugbot_check, ChecksRollup::Pending) {
        ChecksRollup::Pending
    } else {
        bugbot_check
    };

    Some(GithubPrStatus {
        pr_number: number,
        url,
        state,
        is_draft,
        checks,
        bugbot,
        has_merge_conflicts,
        review_decision,
        unresolved_threads,
        head_branch: pr
            .get("headRefName")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        stack: parse_stack(pr),
        head_sha,
        fetched_at: Utc::now(),
        last_error: None,
    })
}

/// Projects GraphQL's `stack` + `stackEntry` into one value. All three numbers
/// have to be there: a stack we can't position the PR within would group chips
/// without being able to order them.
fn parse_stack(pr: &Value) -> Option<PrStack> {
    let u32_at = |v: Option<&Value>| v.and_then(Value::as_u64).map(|n| n as u32);
    let stack = pr.get("stack")?;
    Some(PrStack {
        number: u32_at(stack.get("number"))?,
        size: u32_at(stack.get("size"))?,
        position: u32_at(pr.get("stackEntry").and_then(|e| e.get("position")))?,
    })
}

fn rollup_state_from_str(s: &str) -> ChecksRollup {
    match s {
        "SUCCESS" => ChecksRollup::Success,
        "FAILURE" | "ERROR" => ChecksRollup::Failure,
        "PENDING" | "EXPECTED" => ChecksRollup::Pending,
        _ => ChecksRollup::Neutral,
    }
}

fn context_is_bugbot(node: &Value) -> bool {
    let name = node
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| node.get("context").and_then(|v| v.as_str()))
        .unwrap_or("");
    name.to_lowercase().contains("bugbot")
}

/// GitHub login used by Cursor Bugbot to post reviews and threads.
const BUGBOT_LOGIN: &str = "cursor";

/// Derive a verdict from the reviews themselves, for when GitHub declines to
/// compute a `reviewDecision`. `latestOpinionatedReviews` is already one
/// APPROVED/CHANGES_REQUESTED per reviewer, so this only has to pick a winner:
/// a block outranks an approval. Bugbot is skipped — it drives its own square,
/// and a bot shouldn't read as a human blocking the PR.
fn review_decision_from_reviews(pr: &Value) -> ReviewDecision {
    let Some(nodes) = pr
        .get("latestOpinionatedReviews")
        .and_then(|r| r.get("nodes"))
        .and_then(|n| n.as_array())
    else {
        return ReviewDecision::None;
    };

    let mut approved = false;
    for node in nodes {
        if review_author(node) == Some(BUGBOT_LOGIN) {
            continue;
        }
        match node.get("state").and_then(|s| s.as_str()) {
            Some("CHANGES_REQUESTED") => return ReviewDecision::ChangesRequested,
            Some("APPROVED") => approved = true,
            _ => {}
        }
    }

    if approved {
        ReviewDecision::Approved
    } else {
        ReviewDecision::None
    }
}

fn review_author(review: &Value) -> Option<&str> {
    review
        .get("author")
        .and_then(|a| a.get("login"))
        .and_then(|l| l.as_str())
}

fn thread_first_author(thread: &Value) -> Option<&str> {
    thread
        .get("comments")
        .and_then(|c| c.get("nodes"))
        .and_then(|n| n.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("author"))
        .and_then(|a| a.get("login"))
        .and_then(|l| l.as_str())
}

/// Map a single check-run / status-context node to a rollup-style state.
/// Mirrors GitHub's own aggregation: any incomplete check is `Pending`;
/// completed checks use their `conclusion`.
fn context_state(node: &Value) -> Option<ChecksRollup> {
    let typename = node.get("__typename").and_then(|v| v.as_str())?;
    match typename {
        "CheckRun" => {
            let status = node.get("status").and_then(|s| s.as_str()).unwrap_or("");
            if status != "COMPLETED" {
                return Some(ChecksRollup::Pending);
            }
            match node.get("conclusion").and_then(|c| c.as_str())? {
                "SUCCESS" => Some(ChecksRollup::Success),
                "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED"
                | "CANCELLED" | "STALE" => Some(ChecksRollup::Failure),
                "NEUTRAL" | "SKIPPED" => Some(ChecksRollup::Neutral),
                _ => None,
            }
        }
        "StatusContext" => match node.get("state").and_then(|s| s.as_str())? {
            "SUCCESS" => Some(ChecksRollup::Success),
            "FAILURE" | "ERROR" => Some(ChecksRollup::Failure),
            "PENDING" | "EXPECTED" => Some(ChecksRollup::Pending),
            _ => None,
        },
        _ => None,
    }
}

fn aggregate_rollup(states: impl Iterator<Item = ChecksRollup>) -> ChecksRollup {
    let mut has_failure = false;
    let mut has_pending = false;
    let mut has_success = false;
    let mut has_neutral = false;
    for s in states {
        match s {
            ChecksRollup::Failure => has_failure = true,
            ChecksRollup::Pending => has_pending = true,
            ChecksRollup::Success => has_success = true,
            ChecksRollup::Neutral => has_neutral = true,
            ChecksRollup::None => {}
        }
    }
    if has_failure {
        ChecksRollup::Failure
    } else if has_pending {
        ChecksRollup::Pending
    } else if has_success {
        ChecksRollup::Success
    } else if has_neutral {
        ChecksRollup::Neutral
    } else {
        ChecksRollup::None
    }
}

/// Apply parsed results to `AppState`, returning the set of changes to emit.
pub fn apply_results(state: &mut AppState, results: &[PollResult]) -> Vec<PollResult> {
    let mut changed = Vec::new();
    for result in results {
        let Some(ws) = state.find_workspace_mut(&result.workspace_id) else {
            continue;
        };
        let Some(link) = ws.link_mut(&result.repo_key) else {
            continue;
        };
        let slot = match result.kind {
            TargetKind::Branch(_) => &mut link.github,
            // A PR the user detached mid-tick has no slot left to write to.
            TargetKind::Pr(number) => {
                match link.attached_prs.iter_mut().find(|a| a.number == number) {
                    Some(attached) => &mut attached.status,
                    None => continue,
                }
            }
        };
        let meaningful = is_meaningful_change(slot.as_ref(), result.status.as_ref());
        // Store every poll, even a no-op one, so `fetched_at` tracks when we
        // last heard from GitHub rather than when the PR last changed. The
        // UI fades a status once it goes stale — that's meant to flag a
        // wedged poller, not a PR nobody has touched in a day.
        *slot = result.status.clone();
        if meaningful {
            changed.push(result.clone());
        }
    }
    changed
}

/// Compare two statuses ignoring `fetched_at`. Emit only on real changes so
/// we don't spam the UI every 45s with "nothing changed, but the timestamp
/// did".
fn is_meaningful_change(old: Option<&GithubPrStatus>, new: Option<&GithubPrStatus>) -> bool {
    match (old, new) {
        (None, None) => false,
        (None, Some(_)) | (Some(_), None) => true,
        (Some(a), Some(b)) => {
            a.pr_number != b.pr_number
                || a.url != b.url
                || a.state != b.state
                || a.is_draft != b.is_draft
                || a.checks != b.checks
                || a.bugbot != b.bugbot
                || a.has_merge_conflicts != b.has_merge_conflicts
                || a.review_decision != b.review_decision
                || a.unresolved_threads != b.unresolved_threads
                || a.head_branch != b.head_branch
                || a.stack != b.stack
                || a.head_sha != b.head_sha
                || a.last_error != b.last_error
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_target(i: usize) -> Target {
        mk_target_kind(i, TargetKind::Branch(format!("feat/foo-{i}")))
    }

    fn mk_target_kind(i: usize, kind: TargetKind) -> Target {
        Target {
            workspace_id: format!("ws-{i}"),
            repo_key: "frontend".to_string(),
            slug: GithubSlug {
                owner: "rynobax".to_string(),
                name: "tethys".to_string(),
            },
            kind,
        }
    }

    /// Parse a single-branch-target response and hand back just the status.
    fn parse_one(data: &Value) -> Option<GithubPrStatus> {
        parse_response(&[mk_target(0)], data)
            .into_iter()
            .next()
            .expect("one result per target")
            .status
    }

    #[test]
    fn query_builds_aliases_and_variables() {
        let targets = vec![mk_target(0), mk_target(1)];
        let (q, vars) = build_query(&targets);
        assert!(q.contains("q0: repository(owner: $q0_owner"));
        assert!(q.contains("q1: repository(owner: $q1_owner"));
        assert!(q.contains("mergedPrs: pullRequests(headRefName: $q0_branch_name"));
        assert_eq!(vars.get("q0_owner").unwrap(), "rynobax");
        assert_eq!(vars.get("q0_branch").unwrap(), "refs/heads/feat/foo-0");
        assert_eq!(vars.get("q0_branch_name").unwrap(), "feat/foo-0");
        assert_eq!(vars.get("q1_branch").unwrap(), "refs/heads/feat/foo-1");
        assert_eq!(vars.get("q1_branch_name").unwrap(), "feat/foo-1");
    }

    #[test]
    fn parse_no_branch_returns_none() {
        let data = json!({ "q0": { "ref": null } });
        let parsed = parse_response(&[mk_target(0)], &data);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].status.is_none());
    }

    #[test]
    fn parse_no_pr_returns_none() {
        let data = json!({
            "q0": {
                "ref": { "associatedPullRequests": { "nodes": [] } }
            }
        });
        assert!(parse_one(&data).is_none());
    }

    #[test]
    fn parse_open_pr_with_checks() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 42,
                            "url": "https://github.com/rynobax/tethys/pull/42",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewThreads": {
                                "nodes": [
                                    {"isResolved": false},
                                    {"isResolved": true},
                                    {"isResolved": false}
                                ]
                            },
                            "commits": {
                                "nodes": [{"commit": {
                                    "oid": "abc123",
                                    "statusCheckRollup": {"state": "FAILURE"}
                                }}]
                            }
                        }]
                    }
                }
            }
        });
        let status = parse_one(&data).expect("should parse");
        assert_eq!(status.pr_number, 42);
        assert_eq!(status.state, PrState::Open);
        assert_eq!(status.checks, ChecksRollup::Failure);
        assert_eq!(status.unresolved_threads, 2);
        assert_eq!(status.head_sha, "abc123");
    }

    #[test]
    fn parse_falls_back_to_merged_prs_when_ref_null() {
        // Branch deleted post-merge: GitHub returns `ref: null`, but the PR
        // record is still reachable via pullRequests(headRefName:).
        let data = json!({
            "q0": {
                "ref": null,
                "mergedPrs": {
                    "nodes": [{
                        "number": 99,
                        "url": "https://github.com/x/y/pull/99",
                        "state": "MERGED",
                        "isDraft": false,
                        "reviewThreads": { "nodes": [] },
                        "commits": {
                            "nodes": [{"commit": {"oid": "deadbeef", "statusCheckRollup": null}}]
                        }
                    }]
                }
            }
        });
        let status = parse_one(&data).expect("should fall back to mergedPrs");
        assert_eq!(status.pr_number, 99);
        assert_eq!(status.state, PrState::Merged);
    }

    #[test]
    fn parse_prefers_ref_over_merged_prs_fallback() {
        // Branch still live with an open PR — ignore any older merged PRs
        // that happen to share the branch name.
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 5,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                },
                "mergedPrs": {
                    "nodes": [{
                        "number": 3,
                        "url": "u",
                        "state": "MERGED",
                        "isDraft": false,
                        "reviewThreads": {"nodes": []},
                        "commits": {
                            "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                        }
                    }]
                }
            }
        });
        let status = parse_one(&data).unwrap();
        assert_eq!(status.pr_number, 5);
        assert_eq!(status.state, PrState::Open);
    }

    #[test]
    fn parse_merged_pr_zeroes_unresolved() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 7,
                            "url": "https://github.com/x/y/pull/7",
                            "state": "MERGED",
                            "isDraft": false,
                            "reviewThreads": {
                                "nodes": [{"isResolved": false}, {"isResolved": false}]
                            },
                            "commits": {
                                "nodes": [{"commit": {"oid": "z", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let status = parse_one(&data).expect("parse");
        assert_eq!(status.state, PrState::Merged);
        assert_eq!(status.unresolved_threads, 0);
        assert_eq!(status.checks, ChecksRollup::None);
    }

    #[test]
    fn parse_null_rollup_maps_to_none() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": true,
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let status = parse_one(&data).expect("parse");
        assert_eq!(status.checks, ChecksRollup::None);
        assert!(status.is_draft);
    }

    #[test]
    fn parse_mergeable_conflicting_sets_flag() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "mergeable": "CONFLICTING",
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert!(s.has_merge_conflicts);
    }

    #[test]
    fn parse_mergeable_unknown_does_not_set_flag() {
        // GitHub returns UNKNOWN briefly after a push while it recomputes the
        // merge — don't flash a false positive in that window.
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "mergeable": "UNKNOWN",
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert!(!s.has_merge_conflicts);
    }

    #[test]
    fn parse_mergeable_clean_does_not_set_flag() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "mergeable": "MERGEABLE",
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert!(!s.has_merge_conflicts);
    }

    #[test]
    fn is_meaningful_change_ignores_fetched_at() {
        let base = GithubPrStatus {
            pr_number: 1,
            url: "u".into(),
            state: PrState::Open,
            is_draft: false,
            checks: ChecksRollup::Success,
            bugbot: ChecksRollup::None,
            has_merge_conflicts: false,
            review_decision: ReviewDecision::None,
            unresolved_threads: 0,
            head_branch: Some("feat/foo".into()),
            stack: None,
            head_sha: "sha".into(),
            fetched_at: Utc::now(),
            last_error: None,
        };
        let mut later = base.clone();
        later.fetched_at = Utc::now() + chrono::Duration::seconds(60);
        assert!(!is_meaningful_change(Some(&base), Some(&later)));

        let mut changed = base.clone();
        changed.unresolved_threads = 1;
        assert!(is_meaningful_change(Some(&base), Some(&changed)));

        let mut approved = base.clone();
        approved.review_decision = ReviewDecision::Approved;
        assert!(is_meaningful_change(Some(&base), Some(&approved)));
    }

    #[test]
    fn parse_splits_bugbot_from_checks_rollup() {
        // Bugbot says FAILURE, the rest of CI is SUCCESS. The top-level rollup
        // would be FAILURE, but `checks` should reflect non-bugbot only.
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {
                                    "oid": "o",
                                    "statusCheckRollup": {
                                        "state": "FAILURE",
                                        "contexts": {"nodes": [
                                            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                                            {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS"},
                                            {"__typename": "CheckRun", "name": "Cursor Bugbot", "status": "COMPLETED", "conclusion": "FAILURE"}
                                        ]}
                                    }
                                }}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.checks, ChecksRollup::Success);
        assert_eq!(s.bugbot, ChecksRollup::Failure);
    }

    #[test]
    fn parse_bugbot_pending_when_in_progress() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {
                                    "oid": "o",
                                    "statusCheckRollup": {
                                        "state": "PENDING",
                                        "contexts": {"nodes": [
                                            {"__typename": "CheckRun", "name": "Cursor Bugbot", "status": "IN_PROGRESS", "conclusion": null}
                                        ]}
                                    }
                                }}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.bugbot, ChecksRollup::Pending);
        assert_eq!(s.checks, ChecksRollup::None);
    }

    #[test]
    fn parse_unresolved_bugbot_thread_marks_failure() {
        // Cursor Bugbot's CheckRun reports NEUTRAL even when it found bugs —
        // the actionable signal is the unresolved review thread it leaves.
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewThreads": {"nodes": [
                                {
                                    "isResolved": false,
                                    "comments": {"nodes": [{"author": {"login": "cursor"}}]}
                                }
                            ]},
                            "commits": {
                                "nodes": [{"commit": {
                                    "oid": "abc123",
                                    "statusCheckRollup": {
                                        "state": "SUCCESS",
                                        "contexts": {"nodes": [
                                            {"__typename": "CheckRun", "name": "Cursor Bugbot", "status": "COMPLETED", "conclusion": "NEUTRAL"}
                                        ]}
                                    }
                                }}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.bugbot, ChecksRollup::Failure);
    }

    #[test]
    fn parse_resolved_bugbot_thread_does_not_mark_failure() {
        // Once the user resolves bugbot's thread, the square should clear and
        // fall back to the underlying check state.
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewThreads": {"nodes": [
                                {
                                    "isResolved": true,
                                    "comments": {"nodes": [{"author": {"login": "cursor"}}]}
                                }
                            ]},
                            "commits": {
                                "nodes": [{"commit": {
                                    "oid": "abc123",
                                    "statusCheckRollup": {
                                        "state": "SUCCESS",
                                        "contexts": {"nodes": [
                                            {"__typename": "CheckRun", "name": "Cursor Bugbot", "status": "COMPLETED", "conclusion": "NEUTRAL"}
                                        ]}
                                    }
                                }}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.bugbot, ChecksRollup::Neutral);
    }

    #[test]
    fn parse_bugbot_threads_excluded_from_unresolved_count() {
        // Bugbot leaves its findings as review threads. They shouldn't tip the
        // human-review indicator yellow — bugbot has its own square.
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewDecision": "APPROVED",
                            "reviewThreads": {"nodes": [
                                {
                                    "isResolved": false,
                                    "comments": {"nodes": [{"author": {"login": "cursor"}}]}
                                },
                                {
                                    "isResolved": false,
                                    "comments": {"nodes": [{"author": {"login": "alice"}}]}
                                }
                            ]},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.unresolved_threads, 1);
    }

    #[test]
    fn parse_falls_back_to_top_level_rollup_when_no_contexts() {
        // Older fixture shape — no contexts list. We still get the legacy rollup
        // for `checks`, and `bugbot` falls through to None.
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": {"state": "SUCCESS"}}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.checks, ChecksRollup::Success);
        assert_eq!(s.bugbot, ChecksRollup::None);
    }

    #[test]
    fn parse_review_decision_open() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewDecision": "APPROVED",
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": {"state": "SUCCESS"}}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.review_decision, ReviewDecision::Approved);
    }

    #[test]
    fn parse_review_decision_null_maps_to_none() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewDecision": null,
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.review_decision, ReviewDecision::None);
    }

    /// An open PR whose `reviewDecision` is `decision`, carrying `reviews` as
    /// its latest opinionated reviews.
    fn reviewed_pr(decision: Value, reviews: Value) -> Value {
        json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "reviewDecision": decision,
                            "latestOpinionatedReviews": {"nodes": reviews},
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        })
    }

    fn review(state: &str, login: &str) -> Value {
        json!({"state": state, "author": {"login": login}})
    }

    /// Repos without required reviews get a null `reviewDecision` from GitHub
    /// no matter how many approvals land, so the approval has to come from the
    /// reviews themselves.
    #[test]
    fn parse_review_decision_falls_back_to_approval_when_null() {
        let data = reviewed_pr(Value::Null, json!([review("APPROVED", "christianbundy")]));
        let s = parse_one(&data).unwrap();
        assert_eq!(s.review_decision, ReviewDecision::Approved);
    }

    #[test]
    fn parse_review_decision_fallback_blocks_over_approval() {
        let data = reviewed_pr(
            Value::Null,
            json!([
                review("APPROVED", "alice"),
                review("CHANGES_REQUESTED", "bob"),
            ]),
        );
        let s = parse_one(&data).unwrap();
        assert_eq!(s.review_decision, ReviewDecision::ChangesRequested);
    }

    /// Bugbot has its own square — its verdict must not move the human one.
    #[test]
    fn parse_review_decision_fallback_ignores_bugbot() {
        let data = reviewed_pr(
            Value::Null,
            json!([review("CHANGES_REQUESTED", BUGBOT_LOGIN)]),
        );
        let s = parse_one(&data).unwrap();
        assert_eq!(s.review_decision, ReviewDecision::None);
    }

    /// When GitHub does compute a decision it accounts for CODEOWNERS and
    /// required-approval counts, so it outranks anything we'd infer.
    #[test]
    fn parse_review_decision_prefers_github_verdict() {
        let data = reviewed_pr(
            json!("REVIEW_REQUIRED"),
            json!([review("APPROVED", "alice")]),
        );
        let s = parse_one(&data).unwrap();
        assert_eq!(s.review_decision, ReviewDecision::ReviewRequired);
    }

    #[test]
    fn parse_review_decision_zero_on_merged() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "MERGED",
                            "isDraft": false,
                            "reviewDecision": "APPROVED",
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.review_decision, ReviewDecision::None);
    }

    #[test]
    fn parse_captures_head_branch() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "headRefName": "feat/foo-0",
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(s.head_branch.as_deref(), Some("feat/foo-0"));
        // A PR outside a `gh stack` reports no stack at all — which is what
        // keeps the UI from grouping hand-based PRs.
        assert_eq!(s.stack, None);
    }

    #[test]
    fn parse_captures_gh_stack_membership() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 4240,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "headRefName": "mui-autonext-queue-item",
                            "stack": {"number": 4245, "size": 6},
                            "stackEntry": {"position": 2},
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        let s = parse_one(&data).unwrap();
        assert_eq!(
            s.stack,
            Some(PrStack {
                number: 4245,
                size: 6,
                position: 2
            })
        );
    }

    /// A stack we can't place the PR within can't be ordered, so it's no more
    /// use than no stack at all.
    #[test]
    fn parse_drops_a_stack_with_no_position() {
        let data = json!({
            "q0": {
                "ref": {
                    "associatedPullRequests": {
                        "nodes": [{
                            "number": 1,
                            "url": "u",
                            "state": "OPEN",
                            "isDraft": false,
                            "headRefName": "feat/foo-0",
                            "stack": {"number": 9, "size": 2},
                            "stackEntry": null,
                            "reviewThreads": {"nodes": []},
                            "commits": {
                                "nodes": [{"commit": {"oid": "o", "statusCheckRollup": null}}]
                            }
                        }]
                    }
                }
            }
        });
        assert_eq!(parse_one(&data).unwrap().stack, None);
    }

    #[test]
    fn pr_target_query_uses_pull_request_by_number() {
        let targets = vec![mk_target_kind(0, TargetKind::Pr(512))];
        let (q, vars) = build_query(&targets);
        assert!(q.contains("q0: repository(owner: $q0_owner, name: $q0_name)"));
        assert!(q.contains("pullRequest(number: 512)"));
        // A number-targeted query has no branch to look up.
        assert!(!q.contains("$q0_branch"));
        assert!(!vars.contains_key("q0_branch"));
        assert_eq!(vars.get("q0_name").unwrap(), "tethys");
    }

    #[test]
    fn parse_pr_target_reads_pull_request_node() {
        let data = json!({
            "q0": {
                "pullRequest": {
                    "number": 512,
                    "url": "https://github.com/rynobax/tethys/pull/512",
                    "state": "OPEN",
                    "isDraft": false,
                    "headRefName": "feat/second-branch",
                    "reviewThreads": {"nodes": [{"isResolved": false}]},
                    "commits": {
                        "nodes": [{"commit": {
                            "oid": "sha512",
                            "statusCheckRollup": {"state": "SUCCESS"}
                        }}]
                    }
                }
            }
        });
        let target = mk_target_kind(0, TargetKind::Pr(512));
        let result = parse_response(&[target], &data).remove(0);
        let status = result.status.expect("should parse");
        assert_eq!(status.pr_number, 512);
        assert_eq!(status.head_branch.as_deref(), Some("feat/second-branch"));
        assert_eq!(status.checks, ChecksRollup::Success);
        assert_eq!(status.unresolved_threads, 1);
    }

    #[test]
    fn parse_missing_pr_target_returns_none() {
        // A detached-or-bogus number comes back as `pullRequest: null`.
        let data = json!({ "q0": { "pullRequest": null } });
        let target = mk_target_kind(0, TargetKind::Pr(999));
        let result = parse_response(&[target], &data).remove(0);
        assert!(result.status.is_none());
    }

    fn mk_status(number: u32) -> GithubPrStatus {
        GithubPrStatus {
            pr_number: number,
            url: format!("https://github.com/rynobax/tethys/pull/{number}"),
            state: PrState::Open,
            is_draft: false,
            checks: ChecksRollup::Success,
            bugbot: ChecksRollup::None,
            has_merge_conflicts: false,
            review_decision: ReviewDecision::None,
            unresolved_threads: 0,
            head_branch: None,
            stack: None,
            head_sha: "sha".into(),
            fetched_at: Utc::now(),
            last_error: None,
        }
    }

    fn mk_state_with_attached(number: u32) -> AppState {
        AppState {
            workspaces: vec![crate::state::Workspace {
                id: "ws-0".into(),
                branch: "feat/foo-0".into(),
                created_at: Utc::now(),
                repo_links: vec![crate::state::RepoLink {
                    repo_key: "frontend".into(),
                    worktree_path: "/tmp/wt/frontend".into(),
                    setup_script_ran_at: None,
                    github: None,
                    attached_prs: vec![crate::state::AttachedPr {
                        number,
                        attached_at: Utc::now(),
                        status: None,
                    }],
                    created_branch: true,
                }],
                sessions: Vec::new(),
                claude_binary: None,
                origin: crate::state::Origin::Ui,
                deleted_at: None,
                folder: None,
                status: Default::default(),
                script_runs: Vec::new(),
                notes: String::new(),
            blocked_by: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn apply_writes_pr_result_to_the_attached_slot() {
        let mut state = mk_state_with_attached(512);
        let results = vec![
            PollResult {
                workspace_id: "ws-0".into(),
                repo_key: "frontend".into(),
                kind: TargetKind::Branch("feat/foo-0".into()),
                status: Some(mk_status(10)),
            },
            PollResult {
                workspace_id: "ws-0".into(),
                repo_key: "frontend".into(),
                kind: TargetKind::Pr(512),
                status: Some(mk_status(512)),
            },
        ];
        let changed = apply_results(&mut state, &results);
        assert_eq!(changed.len(), 2);

        let link = &state.workspaces[0].repo_links[0];
        // The branch PR and the attached PR land in separate slots.
        assert_eq!(link.github.as_ref().unwrap().pr_number, 10);
        assert_eq!(link.attached_prs[0].status.as_ref().unwrap().pr_number, 512);
    }

    #[test]
    fn apply_ignores_pr_result_with_no_matching_attachment() {
        // The user detached the PR between building the query and applying it.
        let mut state = mk_state_with_attached(512);
        let results = vec![PollResult {
            workspace_id: "ws-0".into(),
            repo_key: "frontend".into(),
            kind: TargetKind::Pr(777),
            status: Some(mk_status(777)),
        }];
        let changed = apply_results(&mut state, &results);
        assert!(changed.is_empty());
        assert!(state.workspaces[0].repo_links[0].attached_prs[0]
            .status
            .is_none());
    }

    /// A PR nobody has touched still has to look freshly polled, or the UI
    /// fades it as stale while the poller is working perfectly.
    #[test]
    fn apply_advances_fetched_at_without_emitting() {
        let mut state = mk_state_with_attached(512);
        let mut first = mk_status(512);
        first.fetched_at = Utc::now() - chrono::Duration::hours(6);
        state.workspaces[0].repo_links[0].attached_prs[0].status = Some(first.clone());

        let mut polled = first.clone();
        polled.fetched_at = Utc::now();
        let results = vec![PollResult {
            workspace_id: "ws-0".into(),
            repo_key: "frontend".into(),
            kind: TargetKind::Pr(512),
            status: Some(polled.clone()),
        }];

        // Nothing about the PR changed, so the frontend hears nothing...
        let changed = apply_results(&mut state, &results);
        assert!(changed.is_empty());
        // ...but the timestamp still moves.
        let stored = state.workspaces[0].repo_links[0].attached_prs[0]
            .status
            .as_ref()
            .unwrap();
        assert_eq!(stored.fetched_at, polled.fetched_at);
        assert!(stored.fetched_at > first.fetched_at);
    }

    #[test]
    fn pr_result_event_carries_the_number() {
        let result = PollResult {
            workspace_id: "ws-0".into(),
            repo_key: "frontend".into(),
            kind: TargetKind::Pr(512),
            status: None,
        };
        assert_eq!(result.event()["pr_number"], json!(512));

        let branch = PollResult {
            kind: TargetKind::Branch("feat/foo-0".into()),
            ..result
        };
        assert_eq!(branch.event()["pr_number"], Value::Null);
    }
}
