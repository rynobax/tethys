//! A parent workspace is told when one of its children is deleted, so an
//! agent handing out work can move on without being told by hand.
//!
//! An event is queued on the parent before anything tries to deliver it, and
//! leaves the queue only once delivered: pushed into the parent's Claude
//! inbox, which wakes an idle session, or failing that returned from the
//! parent's next replying hook.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tracing::{debug, info, warn};

use crate::agent::Agent;
use crate::github::poller::fetch_pr_status;
use crate::github::status::PrState;
use crate::github::GithubPrStatus;
use crate::registry::RegistryLoad;
use crate::state::{Inbox, Workspace};
use crate::store::Store;

const POST_TIMEOUT: Duration = Duration::from_secs(2);

/// Takes the child as it was at deletion: its PRs are read off this copy.
pub async fn child_deleted(store: &Store, registry: &RegistryLoad, child: Workspace) {
    let Some(parent_id) = child.parent.clone() else {
        return;
    };
    let parent_agent = store
        .read(|s| s.find_workspace(&parent_id).map(|w| w.agent))
        .await;
    match parent_agent {
        Some(Agent::Claude) => {}
        // Nothing delivers to codex yet, so queuing would only pile up.
        Some(Agent::Codex) => {
            debug!(parent = %parent_id, "child deleted under a codex parent; not told");
            return;
        }
        None => return,
    }

    let event = describe(&child, &pr_lines(registry, &child).await);
    let queued = store
        .update_workspace_quiet(&parent_id, |ws| {
            ws.child_events.push(event);
            Ok(())
        })
        .await;
    if let Err(e) = queued {
        warn!(parent = %parent_id, error = %e, "couldn't queue child event");
        return;
    }
    info!(parent = %parent_id, child = %child.id, "queued child-deleted event");
    deliver(store, &parent_id).await;
}

/// Leaves the queue alone when there's no inbox to post to or the post fails.
pub async fn deliver(store: &Store, parent_id: &str) {
    let taken = store
        .update_workspace_quiet(parent_id, |ws| {
            let Some(inbox) = ws.session.as_ref().and_then(|s| s.inbox.clone()) else {
                return Ok(None);
            };
            if ws.agent != Agent::Claude || ws.child_events.is_empty() {
                return Ok(None);
            }
            Ok(Some((inbox, std::mem::take(&mut ws.child_events))))
        })
        .await;
    let Ok(Some((inbox, events))) = taken else {
        return;
    };

    match post(&inbox, &events.join("\n\n")).await {
        Ok(()) => info!(parent = %parent_id, count = events.len(), "posted child events to inbox"),
        Err(e) => {
            debug!(parent = %parent_id, error = %e, "inbox post failed; events wait for a hook");
            let _ = store
                .update_workspace_quiet(parent_id, |ws| {
                    ws.child_events.splice(0..0, events);
                    Ok(())
                })
                .await;
        }
    }
}

/// For a hook reply. Claude only: codex hooks aren't known to carry context.
pub async fn take_for_reply(store: &Store, workspace_id: &str) -> Option<String> {
    let waiting = store
        .read(|s| {
            s.find_workspace(workspace_id)
                .is_some_and(|w| !w.child_events.is_empty())
        })
        .await;
    if !waiting {
        return None;
    }
    let events = store
        .update_workspace_quiet(workspace_id, |ws| {
            Ok(if ws.agent == Agent::Claude {
                std::mem::take(&mut ws.child_events)
            } else {
                Vec::new()
            })
        })
        .await
        .ok()?;
    (!events.is_empty()).then(|| events.join("\n\n"))
}

/// Claude Code's own inbox protocol: an auth line, then a user message.
async fn post(inbox: &Inbox, text: &str) -> std::io::Result<()> {
    let auth = serde_json::json!({ "type": "auth", "token": inbox.token });
    let message = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    });
    let payload = format!("{auth}\n{message}\n");
    tokio::time::timeout(POST_TIMEOUT, write_all(&inbox.socket, payload.as_bytes()))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "inbox post timed out"))?
}

async fn write_all(socket: &Path, payload: &[u8]) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket).await?;
    stream.write_all(payload).await?;
    stream.shutdown().await
}

/// Detached PRs too: clearing a chip from the sidebar doesn't make the PR
/// irrelevant to whoever handed the work out. Fetched fresh, since a detached
/// PR isn't polled and a tracked one may be a tick behind.
async fn pr_lines(registry: &RegistryLoad, child: &Workspace) -> Vec<String> {
    let Ok(reg) = registry.require() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for link in &child.repo_links {
        let Some(slug) = reg
            .find_repo(&link.repo_key)
            .and_then(|r| r.github_slug.clone())
        else {
            continue;
        };
        let numbers = link
            .prs
            .iter()
            .map(|p| p.number)
            .chain(link.dismissed.iter().copied());
        for number in numbers {
            let fresh = fetch_pr_status(&slug, number).await.ok().flatten();
            let last_seen = link.tracked(number).and_then(|p| p.status.clone());
            let state = fresh
                .or(last_seen)
                .map_or("state unknown", |s| state_word(&s));
            lines.push(format!("{}/{}#{number} {state}", slug.owner, slug.name));
        }
    }
    lines
}

fn state_word(status: &GithubPrStatus) -> &'static str {
    match status.state {
        PrState::Merged => "merged",
        PrState::Closed => "closed",
        PrState::Open if status.is_draft => "open (draft)",
        PrState::Open => "open",
    }
}

fn describe(child: &Workspace, prs: &[String]) -> String {
    let prs = if prs.is_empty() {
        "none".to_string()
    } else {
        prs.join(", ")
    };
    format!(
        "[Tethys] Your child workspace on branch {} was deleted.\nTracked PRs: {prs}",
        child.branch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::test_support;

    #[test]
    fn a_child_without_prs_says_so() {
        let child = test_support::workspace("c", "fix-auth", "frontend");
        assert_eq!(
            describe(&child, &[]),
            "[Tethys] Your child workspace on branch fix-auth was deleted.\nTracked PRs: none"
        );
    }

    #[test]
    fn prs_are_listed_in_one_line() {
        let child = test_support::workspace("c", "fix-auth", "frontend");
        let text = describe(&child, &["o/r#1 merged".into(), "o/r#2 closed".into()]);
        assert!(text.ends_with("Tracked PRs: o/r#1 merged, o/r#2 closed"));
    }

    #[tokio::test]
    async fn a_post_is_an_auth_line_then_a_user_message() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("inbox.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let inbox = Inbox {
            socket,
            token: "tok".into(),
        };

        let (posted, received) = tokio::join!(post(&inbox, "hello"), async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = String::new();
            tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut buf)
                .await
                .unwrap();
            buf
        });
        posted.unwrap();

        let lines: Vec<serde_json::Value> = received
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines[0],
            serde_json::json!({ "type": "auth", "token": "tok" })
        );
        assert_eq!(
            lines[1],
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": "hello" },
            })
        );
    }

    #[test]
    fn a_draft_reads_as_open_draft() {
        let mut status = test_support::status(1);
        status.state = PrState::Open;
        status.is_draft = true;
        assert_eq!(state_word(&status), "open (draft)");
    }
}
