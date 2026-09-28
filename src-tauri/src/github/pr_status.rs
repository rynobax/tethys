//! Build the PR GraphQL query, parse the response, and apply it to `AppState`.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{json, Value};

use crate::github::status::{
    ChecksRollup, GithubPrStatus, MergeQueueState, PrStack, PrState, ReviewDecision,
};
use crate::github::GithubSlug;
use crate::state::{AppState, WorkspaceId};

#[derive(Debug, Clone)]
pub struct Target {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub slug: GithubSlug,
    pub kind: TargetKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    /// Finds which PR number to track; yields no status.
    Branch(String),
    Pr(u32),
}

#[derive(Debug, Clone)]
pub enum PollOutcome {
    Discovered(Option<u32>),
    /// `None`: unreachable, but still tracked.
    Status {
        number: u32,
        status: Option<GithubPrStatus>,
    },
}

#[derive(Debug, Clone)]
pub struct PollResult {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub outcome: PollOutcome,
}

#[derive(Debug, Clone)]
pub struct Discovery {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub number: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Applied {
    pub changed: Vec<PollResult>,
    pub discovered: Vec<Discovery>,
}

impl PollResult {
    /// Payload for `github:status_changed`.
    pub fn event(&self) -> Option<Value> {
        let PollOutcome::Status { number, status } = &self.outcome else {
            return None;
        };
        Some(json!({
            "workspace_id": self.workspace_id,
            "repo_key": self.repo_key,
            "pr_number": number,
            "status": status,
        }))
    }
}

pub fn build_query(targets: &[Target]) -> (String, BTreeMap<String, String>) {
    let mut vars = BTreeMap::new();
    let mut var_decls = Vec::new();
    let mut body = String::new();

    const PR_FIELDS: &str = r#"number
          url
          state
          isDraft
          mergeable
          isInMergeQueue
          mergeQueueEntry {
            state
          }
          headRefName
          stack {
            number
            size
          }
          stackEntry {
            position
          }
          reviewDecision
          reviewRequests(first: 1) {
            totalCount
          }
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
                // Number only: the full selection is fetched per tracked PR.
                // `mergedPrs` covers a branch deleted on merge, whose `ref` is null.
                body.push_str(&format!(
                    r#"q{i}: repository(owner: ${ow}, name: ${nm}) {{
    ref(qualifiedName: ${br}) {{
      associatedPullRequests(first: 1, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
        nodes {{
          number
        }}
      }}
    }}
    mergedPrs: pullRequests(headRefName: ${bn}, states: [MERGED, CLOSED], first: 1, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
      nodes {{
        number
      }}
    }}
  }}
"#
                ));
            }
            // Inlined: `-f` only sends strings and `number` is an `Int!`.
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
        let outcome = match t.kind {
            TargetKind::Branch(_) => PollOutcome::Discovered(node.and_then(parse_branch_pr_number)),
            TargetKind::Pr(number) => PollOutcome::Status {
                number,
                status: node
                    .and_then(|repo| repo.get("pullRequest"))
                    .and_then(parse_pr_node),
            },
        };
        out.push(PollResult {
            workspace_id: t.workspace_id.clone(),
            repo_key: t.repo_key.clone(),
            outcome,
        });
    }
    out
}

fn parse_branch_pr_number(repo: &Value) -> Option<u32> {
    fn first_number(connection: Option<&Value>) -> Option<u32> {
        connection?
            .get("nodes")?
            .as_array()?
            .first()?
            .get("number")?
            .as_u64()
            .map(|n| n as u32)
    }

    let assoc = repo
        .get("ref")
        .and_then(|r| r.get("associatedPullRequests"));
    first_number(assoc).or_else(|| first_number(repo.get("mergedPrs")))
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

    // UNKNOWN is transient while GitHub recomputes after a push.
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
            // Null when the base branch doesn't require reviews.
            _ => review_decision_from_reviews(pr),
        }
    } else {
        ReviewDecision::None
    };

    let review_requested = state == PrState::Open
        && pr
            .get("reviewRequests")
            .and_then(|r| r.get("totalCount"))
            .and_then(|n| n.as_u64())
            .is_some_and(|n| n > 0);

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

    // Bugbot's conclusion can be SUCCESS/NEUTRAL despite findings; its
    // unresolved threads are the real signal.
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
        review_requested,
        unresolved_threads,
        head_branch: pr
            .get("headRefName")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        stack: parse_stack(pr),
        merge_queue: parse_merge_queue(pr),
        head_sha,
        fetched_at: Utc::now(),
        last_error: None,
    })
}

/// `isInMergeQueue` is authoritative; a missing entry still means `Queued`.
fn parse_merge_queue(pr: &Value) -> Option<MergeQueueState> {
    let queued = pr
        .get("isInMergeQueue")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entry_state = pr
        .get("mergeQueueEntry")
        .and_then(|e| e.get("state"))
        .and_then(Value::as_str);
    match entry_state {
        Some("QUEUED") => Some(MergeQueueState::Queued),
        Some("AWAITING_CHECKS") => Some(MergeQueueState::AwaitingChecks),
        Some("MERGEABLE") => Some(MergeQueueState::Mergeable),
        Some("UNMERGEABLE") => Some(MergeQueueState::Unmergeable),
        Some("LOCKED") => Some(MergeQueueState::Locked),
        _ if queued => Some(MergeQueueState::Queued),
        _ => None,
    }
}

/// All three numbers or nothing: an unpositioned stack can't be ordered.
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

const BUGBOT_LOGIN: &str = "cursor";

/// A block outranks an approval. Bugbot has its own indicator, so it's skipped.
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

/// A scan never untracks: tracking ends only at detach.
pub fn apply_results(state: &mut AppState, results: &[PollResult]) -> Applied {
    let mut applied = Applied::default();
    for result in results {
        let Some(ws) = state.find_workspace_mut(&result.workspace_id) else {
            continue;
        };
        let Some(link) = ws.link_mut(&result.repo_key) else {
            continue;
        };
        match &result.outcome {
            PollOutcome::Discovered(found) => {
                let Some(number) = found else { continue };
                if link.discovery_should_skip(*number) {
                    continue;
                }
                link.track(*number, None);
                applied.discovered.push(Discovery {
                    workspace_id: result.workspace_id.clone(),
                    repo_key: result.repo_key.clone(),
                    number: *number,
                });
            }
            PollOutcome::Status { number, status } => {
                // Detached mid-tick.
                let Some(tracked) = link.tracked_mut(*number) else {
                    continue;
                };
                let meaningful = is_meaningful_change(tracked.status.as_ref(), status.as_ref());
                // Stored even when unchanged: staleness should flag a wedged
                // poller, not an idle PR.
                tracked.status = status.clone();
                if meaningful {
                    applied.changed.push(result.clone());
                }
            }
        }
    }
    applied
}

/// Ignores `fetched_at`.
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
                || a.review_requested != b.review_requested
                || a.unresolved_threads != b.unresolved_threads
                || a.head_branch != b.head_branch
                || a.stack != b.stack
                || a.merge_queue != b.merge_queue
                || a.head_sha != b.head_sha
                || a.last_error != b.last_error
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::test_support;

    fn mk_target(i: usize) -> Target {
        mk_target_kind(i, TargetKind::Branch(format!("feat/foo-{i}")))
    }

    fn mk_target_kind(i: usize, kind: TargetKind) -> Target {
        Target {
            workspace_id: format!("ws-{i}"),
            repo_key: "frontend".to_string(),
            slug: GithubSlug::new("rynobax", "tethys"),
            kind,
        }
    }

    /// An open PR node, with `extra`'s keys overriding the defaults.
    fn pr_with(extra: Value) -> Value {
        let mut pr = json!({
            "number": 1,
            "url": "u",
            "state": "OPEN",
            "isDraft": false,
            "headRefName": "feat/foo-0",
            "reviewThreads": {"nodes": []},
            "commits": commit(Value::Null),
        });
        let (Some(pr_obj), Some(extra_obj)) = (pr.as_object_mut(), extra.as_object()) else {
            panic!("both have to be objects");
        };
        for (k, v) in extra_obj {
            pr_obj.insert(k.clone(), v.clone());
        }
        pr
    }

    fn parse(extra: Value) -> GithubPrStatus {
        parse_pr_node(&pr_with(extra)).expect("should parse")
    }

    fn commit(rollup: Value) -> Value {
        json!({"nodes": [{"commit": {"oid": "o", "statusCheckRollup": rollup}}]})
    }

    fn check_run(name: &str, status: &str, conclusion: Option<&str>) -> Value {
        json!({"__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion})
    }

    fn thread(resolved: bool, author: &str) -> Value {
        json!({"isResolved": resolved, "comments": {"nodes": [{"author": {"login": author}}]}})
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

    fn discovered(data: &Value) -> Option<u32> {
        match parse_response(&[mk_target(0)], data)
            .remove(0)
            .outcome
        {
            PollOutcome::Discovered(n) => n,
            PollOutcome::Status { .. } => panic!("a branch target discovers"),
        }
    }

    #[test]
    fn branch_scan_reads_the_associated_pr_number() {
        let data = json!({
            "q0": { "ref": { "associatedPullRequests": { "nodes": [{ "number": 42 }] } } }
        });
        assert_eq!(discovered(&data), Some(42));
    }

    #[test]
    fn branch_scan_falls_back_to_the_merged_pr_number() {
        let data = json!({
            "q0": {
                "ref": null,
                "mergedPrs": { "nodes": [{ "number": 41 }] }
            }
        });
        assert_eq!(discovered(&data), Some(41));
    }

    #[test]
    fn parse_no_branch_returns_none() {
        assert_eq!(discovered(&json!({ "q0": { "ref": null } })), None);
    }

    #[test]
    fn branch_scan_with_no_pr_at_all_discovers_nothing() {
        let data = json!({
            "q0": {
                "ref": { "associatedPullRequests": { "nodes": [] } },
                "mergedPrs": { "nodes": [] }
            }
        });
        assert_eq!(discovered(&data), None);
    }

    #[test]
    fn branch_scan_query_asks_only_for_the_number() {
        let (q, _) = build_query(&[mk_target(0)]);
        assert!(q.contains("associatedPullRequests"));
        assert!(!q.contains("reviewThreads"));
        assert!(!q.contains("statusCheckRollup"));
    }

    #[test]
    fn parse_open_pr_with_checks() {
        let status = parse(json!({
            "number": 42,
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
        }));
        assert_eq!(status.pr_number, 42);
        assert_eq!(status.state, PrState::Open);
        assert_eq!(status.checks, ChecksRollup::Failure);
        assert_eq!(status.unresolved_threads, 2);
        assert_eq!(status.head_sha, "abc123");
    }

    #[test]
    fn parse_merged_pr_zeroes_unresolved() {
        let status = parse(json!({
            "state": "MERGED",
            "reviewThreads": {
                "nodes": [{"isResolved": false}, {"isResolved": false}]
            },
        }));
        assert_eq!(status.state, PrState::Merged);
        assert_eq!(status.unresolved_threads, 0);
        assert_eq!(status.checks, ChecksRollup::None);
    }

    #[test]
    fn parse_null_rollup_maps_to_none() {
        let status = parse(json!({"isDraft": true}));
        assert_eq!(status.checks, ChecksRollup::None);
        assert!(status.is_draft);
    }

    #[test]
    fn parse_mergeable_conflicting_sets_flag() {
        assert!(parse(json!({"mergeable": "CONFLICTING"})).has_merge_conflicts);
    }

    #[test]
    fn parse_mergeable_unknown_does_not_set_flag() {
        assert!(!parse(json!({"mergeable": "UNKNOWN"})).has_merge_conflicts);
    }

    #[test]
    fn parse_mergeable_clean_does_not_set_flag() {
        assert!(!parse(json!({"mergeable": "MERGEABLE"})).has_merge_conflicts);
    }

    #[test]
    fn is_meaningful_change_ignores_fetched_at() {
        let base = test_support::status(1);
        let mut later = base.clone();
        later.fetched_at = Utc::now() + chrono::Duration::seconds(60);
        assert!(!is_meaningful_change(Some(&base), Some(&later)));

        let mut changed = base.clone();
        changed.unresolved_threads = 1;
        assert!(is_meaningful_change(Some(&base), Some(&changed)));

        let mut approved = base.clone();
        approved.review_decision = ReviewDecision::Approved;
        assert!(is_meaningful_change(Some(&base), Some(&approved)));

        let mut requested = base.clone();
        requested.review_requested = true;
        assert!(is_meaningful_change(Some(&base), Some(&requested)));
    }

    #[test]
    fn parse_splits_bugbot_from_checks_rollup() {
        let s = parse(json!({
            "commits": commit(json!({
                "state": "FAILURE",
                "contexts": {"nodes": [
                    check_run("build", "COMPLETED", Some("SUCCESS")),
                    check_run("test", "COMPLETED", Some("SUCCESS")),
                    check_run("Cursor Bugbot", "COMPLETED", Some("FAILURE")),
                ]}
            })),
        }));
        assert_eq!(s.checks, ChecksRollup::Success);
        assert_eq!(s.bugbot, ChecksRollup::Failure);
    }

    #[test]
    fn parse_bugbot_pending_when_in_progress() {
        let s = parse(json!({
            "commits": commit(json!({
                "state": "PENDING",
                "contexts": {"nodes": [check_run("Cursor Bugbot", "IN_PROGRESS", None)]}
            })),
        }));
        assert_eq!(s.bugbot, ChecksRollup::Pending);
        assert_eq!(s.checks, ChecksRollup::None);
    }

    fn bugbot_neutral() -> Value {
        commit(json!({
            "state": "SUCCESS",
            "contexts": {"nodes": [check_run("Cursor Bugbot", "COMPLETED", Some("NEUTRAL"))]}
        }))
    }

    #[test]
    fn parse_unresolved_bugbot_thread_marks_failure() {
        let s = parse(json!({
            "reviewThreads": {"nodes": [thread(false, "cursor")]},
            "commits": bugbot_neutral(),
        }));
        assert_eq!(s.bugbot, ChecksRollup::Failure);
    }

    #[test]
    fn parse_resolved_bugbot_thread_does_not_mark_failure() {
        let s = parse(json!({
            "reviewThreads": {"nodes": [thread(true, "cursor")]},
            "commits": bugbot_neutral(),
        }));
        assert_eq!(s.bugbot, ChecksRollup::Neutral);
    }

    #[test]
    fn parse_bugbot_threads_excluded_from_unresolved_count() {
        let s = parse(json!({
            "reviewDecision": "APPROVED",
            "reviewThreads": {"nodes": [thread(false, "cursor"), thread(false, "alice")]},
        }));
        assert_eq!(s.unresolved_threads, 1);
    }

    #[test]
    fn parse_falls_back_to_top_level_rollup_when_no_contexts() {
        let s = parse(json!({"commits": commit(json!({"state": "SUCCESS"}))}));
        assert_eq!(s.checks, ChecksRollup::Success);
        assert_eq!(s.bugbot, ChecksRollup::None);
    }

    #[test]
    fn parse_review_decision_open() {
        let s = parse(json!({"reviewDecision": "APPROVED"}));
        assert_eq!(s.review_decision, ReviewDecision::Approved);
    }

    fn requested_pr(count: u64) -> Value {
        json!({
            "reviewDecision": "REVIEW_REQUIRED",
            "reviewRequests": {"totalCount": count},
            "latestOpinionatedReviews": {"nodes": []},
        })
    }

    #[test]
    fn parse_review_requested_from_pending_requests() {
        let s = parse(requested_pr(1));
        assert!(s.review_requested);
        assert_eq!(s.review_decision, ReviewDecision::ReviewRequired);

        let s = parse(requested_pr(0));
        assert!(!s.review_requested);
        assert_eq!(s.review_decision, ReviewDecision::ReviewRequired);
    }

    #[test]
    fn parse_review_requested_missing_field_is_false() {
        assert!(!parse(reviewed_pr(Value::Null, json!([]))).review_requested);
    }

    #[test]
    fn parse_review_decision_null_maps_to_none() {
        let s = parse(json!({"reviewDecision": null}));
        assert_eq!(s.review_decision, ReviewDecision::None);
    }

    fn reviewed_pr(decision: Value, reviews: Value) -> Value {
        json!({
            "reviewDecision": decision,
            "latestOpinionatedReviews": {"nodes": reviews},
        })
    }

    fn review(state: &str, login: &str) -> Value {
        json!({"state": state, "author": {"login": login}})
    }

    #[test]
    fn parse_review_decision_falls_back_to_approval_when_null() {
        let s = parse(reviewed_pr(Value::Null, json!([review("APPROVED", "christianbundy")])));
        assert_eq!(s.review_decision, ReviewDecision::Approved);
    }

    #[test]
    fn parse_review_decision_fallback_blocks_over_approval() {
        let s = parse(reviewed_pr(
            Value::Null,
            json!([
                review("APPROVED", "alice"),
                review("CHANGES_REQUESTED", "bob"),
            ]),
        ));
        assert_eq!(s.review_decision, ReviewDecision::ChangesRequested);
    }

    #[test]
    fn parse_review_decision_fallback_ignores_bugbot() {
        let s = parse(reviewed_pr(
            Value::Null,
            json!([review("CHANGES_REQUESTED", BUGBOT_LOGIN)]),
        ));
        assert_eq!(s.review_decision, ReviewDecision::None);
    }

    #[test]
    fn parse_review_decision_prefers_github_verdict() {
        let s = parse(reviewed_pr(
            json!("REVIEW_REQUIRED"),
            json!([review("APPROVED", "alice")]),
        ));
        assert_eq!(s.review_decision, ReviewDecision::ReviewRequired);
    }

    #[test]
    fn parse_review_decision_zero_on_merged() {
        let s = parse(json!({"state": "MERGED", "reviewDecision": "APPROVED"}));
        assert_eq!(s.review_decision, ReviewDecision::None);
    }

    #[test]
    fn parse_captures_head_branch() {
        let s = parse(json!({"headRefName": "feat/bar"}));
        assert_eq!(s.head_branch.as_deref(), Some("feat/bar"));
        assert_eq!(s.stack, None);
    }

    #[test]
    fn parse_captures_gh_stack_membership() {
        let s = parse(json!({
            "number": 4240,
            "stack": {"number": 4245, "size": 6},
            "stackEntry": {"position": 2},
        }));
        assert_eq!(
            s.stack,
            Some(PrStack {
                number: 4245,
                size: 6,
                position: 2
            })
        );
    }

    #[test]
    fn parse_drops_a_stack_with_no_position() {
        let s = parse(json!({
            "stack": {"number": 9, "size": 2},
            "stackEntry": null,
        }));
        assert_eq!(s.stack, None);
    }

    #[test]
    fn parse_captures_merge_queue_state() {
        let s = parse(json!({
            "isInMergeQueue": true,
            "mergeQueueEntry": {"state": "AWAITING_CHECKS"},
        }));
        assert_eq!(s.state, PrState::Open);
        assert_eq!(s.merge_queue, Some(MergeQueueState::AwaitingChecks));
    }

    #[test]
    fn parse_falls_back_to_queued_without_an_entry() {
        let s = parse(json!({
            "isInMergeQueue": true,
            "mergeQueueEntry": null,
        }));
        assert_eq!(s.merge_queue, Some(MergeQueueState::Queued));
    }

    #[test]
    fn parse_reports_no_queue_for_an_unqueued_pr() {
        let s = parse(json!({
            "isInMergeQueue": false,
            "mergeQueueEntry": null,
        }));
        assert_eq!(s.merge_queue, None);
    }

    #[test]
    fn entering_the_merge_queue_is_a_meaningful_change() {
        let before = parse(json!({"isInMergeQueue": false}));
        let mut after = before.clone();
        after.merge_queue = Some(MergeQueueState::Queued);
        assert!(is_meaningful_change(Some(&before), Some(&after)));
    }

    #[test]
    fn pr_target_query_uses_pull_request_by_number() {
        let targets = vec![mk_target_kind(0, TargetKind::Pr(512))];
        let (q, vars) = build_query(&targets);
        assert!(q.contains("q0: repository(owner: $q0_owner, name: $q0_name)"));
        assert!(q.contains("pullRequest(number: 512)"));
        assert!(!q.contains("$q0_branch"));
        assert!(!vars.contains_key("q0_branch"));
        assert_eq!(vars.get("q0_name").unwrap(), "tethys");
    }

    #[test]
    fn parse_pr_target_reads_pull_request_node() {
        let data = json!({
            "q0": {
                "pullRequest": pr_with(json!({
                    "number": 512,
                    "headRefName": "feat/second-branch",
                    "reviewThreads": {"nodes": [{"isResolved": false}]},
                    "commits": commit(json!({"state": "SUCCESS"})),
                }))
            }
        });
        let target = mk_target_kind(0, TargetKind::Pr(512));
        let status = status_of(parse_response(&[target], &data).remove(0)).expect("should parse");
        assert_eq!(status.pr_number, 512);
        assert_eq!(status.head_branch.as_deref(), Some("feat/second-branch"));
        assert_eq!(status.checks, ChecksRollup::Success);
        assert_eq!(status.unresolved_threads, 1);
    }

    #[test]
    fn parse_missing_pr_target_returns_none() {
        let data = json!({ "q0": { "pullRequest": null } });
        let target = mk_target_kind(0, TargetKind::Pr(999));
        assert!(status_of(parse_response(&[target], &data).remove(0)).is_none());
    }

    fn status_of(result: PollResult) -> Option<GithubPrStatus> {
        match result.outcome {
            PollOutcome::Status { status, .. } => status,
            PollOutcome::Discovered(_) => panic!("a Pr target fetches status"),
        }
    }

    fn status_result(number: u32, status: Option<GithubPrStatus>) -> PollResult {
        PollResult {
            workspace_id: "ws-0".into(),
            repo_key: "frontend".into(),
            outcome: PollOutcome::Status { number, status },
        }
    }

    fn discovery_result(number: Option<u32>) -> PollResult {
        PollResult {
            workspace_id: "ws-0".into(),
            repo_key: "frontend".into(),
            outcome: PollOutcome::Discovered(number),
        }
    }

    fn mk_state_tracking(numbers: &[u32]) -> AppState {
        let mut ws = test_support::workspace("ws-0", "feat/foo-0", "frontend");
        for n in numbers {
            ws.repo_links[0].track(*n, None);
        }
        AppState {
            workspaces: vec![ws],
            ..Default::default()
        }
    }

    #[test]
    fn apply_writes_each_status_to_its_own_tracked_pr() {
        let mut state = mk_state_tracking(&[10, 512]);
        let results = vec![
            status_result(10, Some(test_support::status(10))),
            status_result(512, Some(test_support::status(512))),
        ];
        let applied = apply_results(&mut state, &results);
        assert_eq!(applied.changed.len(), 2);

        let link = &state.workspaces[0].repo_links[0];
        assert_eq!(link.tracked(10).unwrap().status.as_ref().unwrap().pr_number, 10);
        assert_eq!(
            link.tracked(512).unwrap().status.as_ref().unwrap().pr_number,
            512
        );
    }

    #[test]
    fn apply_ignores_a_status_for_an_untracked_number() {
        let mut state = mk_state_tracking(&[512]);
        let applied =
            apply_results(&mut state, &[status_result(777, Some(test_support::status(777)))]);
        assert!(applied.changed.is_empty());
        let link = &state.workspaces[0].repo_links[0];
        assert_eq!(link.prs.len(), 1);
        assert!(link.tracked(512).unwrap().status.is_none());
    }

    #[test]
    fn a_scan_starts_tracking_a_new_number() {
        let mut state = mk_state_tracking(&[]);
        let applied = apply_results(&mut state, &[discovery_result(Some(42))]);
        assert_eq!(applied.discovered.len(), 1);
        assert_eq!(applied.discovered[0].number, 42);
        assert!(applied.changed.is_empty());
        let link = &state.workspaces[0].repo_links[0];
        assert_eq!(link.prs.len(), 1);
        assert_eq!(link.prs[0].number, 42);
    }

    #[test]
    fn a_scan_finding_a_pr_we_already_track_does_nothing() {
        let mut state = mk_state_tracking(&[42]);
        let applied = apply_results(&mut state, &[discovery_result(Some(42))]);
        assert!(applied.discovered.is_empty());
        assert_eq!(state.workspaces[0].repo_links[0].prs.len(), 1);
    }

    #[test]
    fn a_scan_does_not_resurrect_a_detached_pr() {
        let mut state = mk_state_tracking(&[42]);
        state.workspaces[0].repo_links[0].untrack(42);
        let applied = apply_results(&mut state, &[discovery_result(Some(42))]);
        assert!(applied.discovered.is_empty());
        assert!(state.workspaces[0].repo_links[0].prs.is_empty());
    }

    #[test]
    fn an_empty_scan_leaves_tracked_prs_alone() {
        let mut state = mk_state_tracking(&[42]);
        apply_results(&mut state, &[status_result(42, Some(test_support::status(42)))]);
        let applied = apply_results(&mut state, &[discovery_result(None)]);
        assert!(applied.discovered.is_empty());
        let link = &state.workspaces[0].repo_links[0];
        assert_eq!(link.prs.len(), 1);
        assert!(link.tracked(42).unwrap().status.is_some());
    }

    #[test]
    fn apply_advances_fetched_at_without_emitting() {
        let mut state = mk_state_tracking(&[512]);
        let mut first = test_support::status(512);
        first.fetched_at = Utc::now() - chrono::Duration::hours(6);
        state.workspaces[0].repo_links[0].prs[0].status = Some(first.clone());

        let mut polled = first.clone();
        polled.fetched_at = Utc::now();

        let applied = apply_results(&mut state, &[status_result(512, Some(polled.clone()))]);
        assert!(applied.changed.is_empty());
        let stored = state.workspaces[0].repo_links[0].prs[0]
            .status
            .as_ref()
            .unwrap();
        assert_eq!(stored.fetched_at, polled.fetched_at);
    }

    #[test]
    fn a_status_event_always_names_its_pr() {
        let result = status_result(512, None);
        assert_eq!(result.event().unwrap()["pr_number"], json!(512));
    }

    #[test]
    fn a_discovery_produces_no_event() {
        assert!(discovery_result(Some(42)).event().is_none());
    }
}
