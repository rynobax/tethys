//! A workspace is **Shipping** once its work is done and its Shipping PRs only
//! need to get merged. Tethys watches those PRs on the poller's tick and
//! Nudges the session about what it can fix alone — a merge conflict, failing
//! CI, a merge-queue failure — waking it if it was closed. Once every Shipping
//! PR is closed the workspace is **Shipped**, or back among the workspaces
//! being worked on if another of its PRs is still open.
//!
//! Nudges are edge-triggered off the poller's old → new status, so a PR that
//! stays red isn't Nudged again every tick; a push that goes red again is.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tracing::{debug, info, warn};

use crate::agent::Agent;
use crate::agent_bin::AgentBins;
use crate::error::{AppError, AppResult};
use crate::github::pr_status::StatusTransition;
use crate::github::status::{ChecksRollup, MergeQueueState, PrState};
use crate::github::GithubPrStatus;
use crate::mcp::McpLaunch;
use crate::paths::Paths;
use crate::sessions::{open_session, OpenSession, SessionSupervisor};
use crate::state::{AppState, PrRef, Shipment, Workspace, WorkspaceId};
use crate::store::Store;

/// Named in every Nudge, so a woken agent knows how to hand back.
pub const LEAVE_TOOL: &str = "mcp__tethys__leave_shipping";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Problem {
    Conflict,
    CiFailing,
    MergeQueue,
}

/// Only what just went wrong: `old` is `None` when there's no history, as when
/// a workspace starts Shipping, and then anything currently wrong counts.
fn problems(old: Option<&GithubPrStatus>, new: &GithubPrStatus) -> Vec<Problem> {
    if new.state != PrState::Open {
        return Vec::new();
    }
    let mut out = Vec::new();
    if new.has_merge_conflicts && !old.is_some_and(|o| o.has_merge_conflicts) {
        out.push(Problem::Conflict);
    }
    if new.checks == ChecksRollup::Failure
        && !old.is_some_and(|o| o.checks == ChecksRollup::Failure)
    {
        out.push(Problem::CiFailing);
    }
    let unmergeable = |s: &GithubPrStatus| s.merge_queue == Some(MergeQueueState::Unmergeable);
    // Leaving without merging is how a failed merge group usually shows up;
    // GitHub drops the entry rather than parking it as unmergeable.
    let dropped_out = old.is_some_and(|o| o.merge_queue.is_some()) && new.merge_queue.is_none();
    if (unmergeable(new) && !old.is_some_and(unmergeable)) || dropped_out {
        out.push(Problem::MergeQueue);
    }
    out
}

struct Finding<'a> {
    pr: &'a PrRef,
    url: &'a str,
    problem: Problem,
}

fn nudge_text(findings: &[Finding<'_>]) -> String {
    let lines: Vec<String> = findings
        .iter()
        .map(|f| {
            let name = format!("{}#{} ({})", f.pr.repo_key, f.pr.number, f.url);
            match f.problem {
                Problem::Conflict => format!(
                    "- {name} has merge conflicts. Rebase it onto its base branch, \
                     resolve them, and force-push."
                ),
                Problem::CiFailing => {
                    format!("- {name} is failing CI. Run /monitor-ci to get it passing.")
                }
                Problem::MergeQueue => format!(
                    "- {name} failed in the merge queue. Find out why (usually a check \
                     that failed on the merge group), fix it, and put it back in the queue."
                ),
            }
        })
        .collect();
    format!(
        "[Tethys] This workspace is Shipping: Ryan considers its work done, and Tethys \
         is watching its PRs until they merge. Something needs fixing:\n\n{}\n\n\
         Fix it without waiting for Ryan. If it needs his input instead (a design \
         question, a review comment he should weigh in on, anything you can't settle \
         alone), call {LEAVE_TOOL} with a one-line reason, which hands the workspace \
         back to him, then explain.",
        lines.join("\n"),
    )
}

/// Queues a Nudge on the workspace if anything's wrong, and says whether it did.
/// Codex sessions have no inbox to hear one through.
fn queue_nudge<'a>(
    ws: &mut Workspace,
    changes: impl IntoIterator<Item = (&'a PrRef, Option<&'a GithubPrStatus>, &'a GithubPrStatus)>,
) -> bool {
    let findings: Vec<Finding<'_>> = changes
        .into_iter()
        .flat_map(|(pr, old, new)| {
            problems(old, new).into_iter().map(move |problem| Finding {
                pr,
                url: &new.url,
                problem,
            })
        })
        .collect();
    if findings.is_empty() {
        return false;
    }
    if ws.agent != Agent::Claude {
        debug!(workspace = %ws.id, "shipping codex workspace not nudged; it has no inbox");
        return false;
    }
    let text = nudge_text(&findings);
    ws.pending_messages.push(text);
    true
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reaction {
    /// A Nudge was queued and wants delivering.
    pub nudged: Vec<WorkspaceId>,
    /// Shipment changed, which the sidebar should hear about.
    pub moved: Vec<WorkspaceId>,
}

/// Runs inside the poller's store write, right after its statuses land.
pub fn react(state: &mut AppState, transitions: &[StatusTransition]) -> Reaction {
    let mut reaction = Reaction::default();
    for ws in state.workspaces.iter_mut() {
        if ws.deleted_at.is_some() || !ws.is_shipping() {
            continue;
        }
        let shipping = ws.shipping_prs().to_vec();
        let changes: Vec<(&PrRef, Option<&GithubPrStatus>, &GithubPrStatus)> = transitions
            .iter()
            .filter(|t| t.workspace_id == ws.id)
            .filter_map(|t| {
                let pr = shipping
                    .iter()
                    .find(|p| p.repo_key == t.repo_key && p.number == t.number)?;
                Some((pr, t.old.as_ref(), t.new.as_ref()?))
            })
            .collect();
        if queue_nudge(ws, changes) {
            reaction.nudged.push(ws.id.clone());
        }
    }
    reaction.moved = settle(state);
    reaction
}

/// Settles every workspace; returns the ones it moved.
pub fn settle(state: &mut AppState) -> Vec<WorkspaceId> {
    state
        .workspaces
        .iter_mut()
        .filter(|ws| ws.deleted_at.is_none())
        .filter_map(|ws| settle_workspace(ws).then(|| ws.id.clone()))
        .collect()
}

/// Drops Shipping PRs that are no longer tracked, and moves the workspace on
/// once they're all closed. Returns whether its shipment changed.
pub fn settle_workspace(ws: &mut Workspace) -> bool {
    let Some(Shipment::Shipping { prs, .. }) = &ws.shipment else {
        return false;
    };
    let still_tracked: Vec<PrRef> = prs
        .iter()
        .filter(|p| ws.tracked_pr(p).is_some())
        .cloned()
        .collect();
    let next = next_shipment(ws, &still_tracked);
    if next == ws.shipment {
        return false;
    }
    info!(workspace = %ws.id, shipment = ?next, "shipment moved");
    ws.shipment = next;
    true
}

fn next_shipment(ws: &Workspace, prs: &[PrRef]) -> Option<Shipment> {
    // Detaching every Shipping PR is calling the shipment off.
    if prs.is_empty() {
        return None;
    }
    let closed = |pr: &PrRef| {
        ws.tracked_pr(pr)
            .and_then(|t| t.status.as_ref())
            .is_some_and(|s| s.state != PrState::Open)
    };
    if !prs.iter().all(closed) {
        let since = match &ws.shipment {
            Some(Shipment::Shipping { since, .. }) => *since,
            _ => Utc::now(),
        };
        return Some(Shipment::Shipping {
            prs: prs.to_vec(),
            since,
        });
    }
    let other_open = ws.repo_links.iter().any(|link| {
        link.prs.iter().any(|t| {
            !prs.iter()
                .any(|p| p.repo_key == link.repo_key && p.number == t.number)
                && t.status.as_ref().is_some_and(|s| s.state == PrState::Open)
        })
    });
    if other_open {
        None
    } else {
        Some(Shipment::Shipped { at: Utc::now() })
    }
}

/// Puts a workspace in Shipping. An empty `prs` means its only open PR, which
/// must then be unambiguous. Returns whether a Nudge was queued for what's
/// already wrong.
pub fn start(ws: &mut Workspace, prs: Vec<PrRef>) -> AppResult<bool> {
    let open: Vec<PrRef> = ws
        .repo_links
        .iter()
        .flat_map(|link| {
            link.prs
                .iter()
                .filter(|t| t.status.as_ref().is_some_and(|s| s.state == PrState::Open))
                .map(|t| PrRef {
                    repo_key: link.repo_key.clone(),
                    number: t.number,
                })
        })
        .collect();
    let prs = if prs.is_empty() {
        match open.as_slice() {
            [] => {
                return Err(AppError::Other(
                    "a workspace needs an open PR before it can ship".into(),
                ))
            }
            [only] => vec![only.clone()],
            _ => {
                return Err(AppError::Other(
                    "this workspace has several open PRs; name the ones shipping".into(),
                ))
            }
        }
    } else {
        prs
    };
    if let Some(stranger) = prs.iter().find(|p| !open.contains(p)) {
        return Err(AppError::Other(format!(
            "{}#{} isn't an open PR this workspace tracks",
            stranger.repo_key, stranger.number
        )));
    }

    ws.shipment = Some(Shipment::Shipping {
        prs: prs.clone(),
        since: Utc::now(),
    });
    info!(workspace = %ws.id, prs = ?prs, "workspace shipping");
    let statuses: Vec<(PrRef, GithubPrStatus)> = prs
        .into_iter()
        .filter_map(|p| {
            let status = ws.tracked_pr(&p)?.status.clone()?;
            Some((p, status))
        })
        .collect();
    Ok(queue_nudge(ws, statuses.iter().map(|(p, s)| (p, None, s))))
}

/// What Nudges a Shipping session: delivers what's queued, resuming the
/// session first if it isn't running, since Shipping sessions are the ones
/// nobody is watching and idle-close takes them first.
pub struct Shipper {
    pub store: Arc<Store>,
    pub supervisor: Arc<SessionSupervisor>,
    /// Empty when tmux didn't resolve at boot.
    pub tmux_bin: PathBuf,
    pub agent_bins: AgentBins,
    pub paths: Paths,
    pub mcp: Option<McpLaunch>,
}

impl Shipper {
    pub async fn wake(&self, workspace_id: &str) {
        let session = self
            .store
            .read(|s| {
                s.find_workspace(workspace_id)
                    .filter(|w| w.deleted_at.is_none())
                    .map(|w| w.session.clone())
            })
            .await;
        let Some(session) = session else { return };
        let running = session
            .and_then(|m| self.supervisor.info(&m.id))
            .is_some_and(|i| i.running);

        if !running {
            if self.tmux_bin.as_os_str().is_empty() {
                return;
            }
            // A fresh session has no inbox until its SessionStart, which
            // delivers the queue itself.
            let opened = open_session(OpenSession {
                supervisor: &self.supervisor,
                store: &self.store,
                workspace_id,
                agent_bins: &self.agent_bins,
                tmux_bin: &self.tmux_bin,
                paths: &self.paths,
                mcp: self.mcp.as_ref(),
                brief: None,
            })
            .await;
            match opened {
                Ok(info) => info!(workspace = %workspace_id, session = %info.id, "woke shipping session for a nudge"),
                Err(e) => {
                    warn!(workspace = %workspace_id, error = %e, "couldn't wake shipping session; nudge waits");
                    return;
                }
            }
        }
        crate::session_inbox::deliver(&self.store, workspace_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::test_support;

    fn pr(number: u32) -> PrRef {
        PrRef {
            repo_key: "api".into(),
            number,
        }
    }

    fn status(number: u32, f: impl FnOnce(&mut GithubPrStatus)) -> GithubPrStatus {
        let mut s = test_support::status(number);
        f(&mut s);
        s
    }

    fn workspace_with(prs: &[GithubPrStatus]) -> Workspace {
        let mut ws = test_support::workspace("ws", "feat/x", "api");
        ws.status = crate::state::WorkspaceStatus::Ready;
        for s in prs {
            ws.repo_links[0].track(s.pr_number, Some(s.clone()));
        }
        ws
    }

    fn shipping(ws: &mut Workspace, prs: &[u32]) {
        ws.shipment = Some(Shipment::Shipping {
            prs: prs.iter().map(|n| pr(*n)).collect(),
            since: Utc::now(),
        });
    }

    fn transition(number: u32, old: GithubPrStatus, new: GithubPrStatus) -> StatusTransition {
        StatusTransition {
            workspace_id: "ws".into(),
            repo_key: "api".into(),
            number,
            old: Some(old),
            new: Some(new),
        }
    }

    #[test]
    fn going_red_is_a_problem_and_staying_red_is_not() {
        let green = status(1, |_| {});
        let red = status(1, |s| s.checks = ChecksRollup::Failure);
        assert_eq!(problems(Some(&green), &red), vec![Problem::CiFailing]);
        assert!(problems(Some(&red), &red).is_empty());
    }

    #[test]
    fn with_no_history_anything_wrong_counts() {
        let bad = status(1, |s| {
            s.checks = ChecksRollup::Failure;
            s.has_merge_conflicts = true;
            s.merge_queue = Some(MergeQueueState::Unmergeable);
        });
        assert_eq!(
            problems(None, &bad),
            vec![Problem::Conflict, Problem::CiFailing, Problem::MergeQueue]
        );
    }

    #[test]
    fn dropping_out_of_the_merge_queue_is_a_failure_but_merging_is_not() {
        let queued = status(1, |s| s.merge_queue = Some(MergeQueueState::AwaitingChecks));
        let dropped = status(1, |_| {});
        let merged = status(1, |s| s.state = PrState::Merged);
        assert_eq!(problems(Some(&queued), &dropped), vec![Problem::MergeQueue]);
        assert!(problems(Some(&queued), &merged).is_empty());
    }

    #[test]
    fn a_shipping_pr_going_red_queues_a_nudge_naming_the_leave_tool() {
        let green = status(1, |_| {});
        let red = status(1, |s| s.checks = ChecksRollup::Failure);
        let mut state = AppState {
            workspaces: vec![workspace_with(std::slice::from_ref(&red))],
            ..Default::default()
        };
        shipping(&mut state.workspaces[0], &[1]);

        let reaction = react(&mut state, &[transition(1, green, red)]);

        assert_eq!(reaction.nudged, vec!["ws".to_string()]);
        let message = &state.workspaces[0].pending_messages[0];
        assert!(message.contains("api#1"), "{message}");
        assert!(message.contains("/monitor-ci"), "{message}");
        assert!(message.contains(LEAVE_TOOL), "{message}");
    }

    #[test]
    fn a_pr_that_is_not_shipping_is_left_alone() {
        let green = status(2, |_| {});
        let red = status(2, |s| s.checks = ChecksRollup::Failure);
        let mut state = AppState {
            workspaces: vec![workspace_with(&[status(1, |_| {}), red.clone()])],
            ..Default::default()
        };
        shipping(&mut state.workspaces[0], &[1]);

        let reaction = react(&mut state, &[transition(2, green, red)]);

        assert!(reaction.nudged.is_empty());
        assert!(state.workspaces[0].pending_messages.is_empty());
    }

    #[test]
    fn a_codex_workspace_is_not_nudged() {
        let green = status(1, |_| {});
        let red = status(1, |s| s.checks = ChecksRollup::Failure);
        let mut ws = workspace_with(std::slice::from_ref(&red));
        ws.agent = Agent::Codex;
        shipping(&mut ws, &[1]);
        let mut state = AppState {
            workspaces: vec![ws],
            ..Default::default()
        };

        assert!(react(&mut state, &[transition(1, green, red)]).nudged.is_empty());
    }

    #[test]
    fn every_shipping_pr_closed_is_shipped() {
        let mut state = AppState {
            workspaces: vec![workspace_with(&[
                status(1, |s| s.state = PrState::Merged),
                status(2, |s| s.state = PrState::Closed),
            ])],
            ..Default::default()
        };
        shipping(&mut state.workspaces[0], &[1, 2]);

        assert_eq!(settle(&mut state), vec!["ws".to_string()]);
        assert!(matches!(
            state.workspaces[0].shipment,
            Some(Shipment::Shipped { .. })
        ));
    }

    #[test]
    fn one_shipping_pr_still_open_keeps_it_shipping() {
        let mut state = AppState {
            workspaces: vec![workspace_with(&[
                status(1, |s| s.state = PrState::Merged),
                status(2, |_| {}),
            ])],
            ..Default::default()
        };
        shipping(&mut state.workspaces[0], &[1, 2]);

        assert!(settle(&mut state).is_empty());
        assert!(state.workspaces[0].is_shipping());
    }

    #[test]
    fn another_open_pr_sends_it_back_rather_than_to_shipped() {
        let mut state = AppState {
            workspaces: vec![workspace_with(&[
                status(1, |s| s.state = PrState::Merged),
                status(2, |_| {}),
            ])],
            ..Default::default()
        };
        shipping(&mut state.workspaces[0], &[1]);

        assert_eq!(settle(&mut state), vec!["ws".to_string()]);
        assert_eq!(state.workspaces[0].shipment, None);
    }

    #[test]
    fn detaching_every_shipping_pr_calls_it_off() {
        let mut state = AppState {
            workspaces: vec![workspace_with(&[status(1, |_| {})])],
            ..Default::default()
        };
        shipping(&mut state.workspaces[0], &[1]);
        state.workspaces[0].repo_links[0].untrack(1);

        assert_eq!(settle(&mut state), vec!["ws".to_string()]);
        assert_eq!(state.workspaces[0].shipment, None);
    }

    #[test]
    fn starting_with_one_open_pr_picks_it() {
        let mut ws = workspace_with(&[status(1, |s| s.state = PrState::Merged), status(2, |_| {})]);
        assert!(!start(&mut ws, Vec::new()).unwrap());
        assert_eq!(ws.shipping_prs(), &[pr(2)]);
    }

    #[test]
    fn starting_with_several_open_prs_needs_them_named() {
        let mut ws = workspace_with(&[status(1, |_| {}), status(2, |_| {})]);
        assert!(start(&mut ws, Vec::new()).is_err());
        start(&mut ws, vec![pr(2)]).unwrap();
        assert_eq!(ws.shipping_prs(), &[pr(2)]);
    }

    #[test]
    fn starting_without_an_open_pr_is_refused() {
        let mut ws = workspace_with(&[status(1, |s| s.state = PrState::Merged)]);
        assert!(start(&mut ws, Vec::new()).is_err());
        assert!(start(&mut ws, vec![pr(1)]).is_err());
        assert_eq!(ws.shipment, None);
    }

    #[test]
    fn starting_on_a_red_pr_nudges_straight_away() {
        let mut ws = workspace_with(&[status(1, |s| s.checks = ChecksRollup::Failure)]);
        assert!(start(&mut ws, Vec::new()).unwrap());
        assert_eq!(ws.pending_messages.len(), 1);
    }
}
