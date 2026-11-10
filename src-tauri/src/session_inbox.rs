//! Messages Tethys owes a workspace's session — news of its children,
//! Nudges — queued on the workspace before anything tries to deliver them.
//! One leaves the queue only once delivered: posted into the session's Claude
//! inbox, which wakes an idle session, or failing that returned from the
//! session's next replying hook.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tracing::{debug, info};

use crate::agent::Agent;
use crate::state::Inbox;
use crate::store::Store;

const POST_TIMEOUT: Duration = Duration::from_secs(2);

/// Leaves the queue alone when there's no inbox to post to or the post fails.
pub async fn deliver(store: &Store, workspace_id: &str) {
    let taken = store
        .update_workspace_quiet(workspace_id, |ws| {
            let Some(inbox) = ws.session.as_ref().and_then(|s| s.inbox.clone()) else {
                return Ok(None);
            };
            if ws.agent != Agent::Claude || ws.pending_messages.is_empty() {
                return Ok(None);
            }
            Ok(Some((inbox, std::mem::take(&mut ws.pending_messages))))
        })
        .await;
    let Ok(Some((inbox, messages))) = taken else {
        return;
    };

    match post(&inbox, &messages.join("\n\n")).await {
        Ok(()) => info!(workspace = %workspace_id, count = messages.len(), "posted pending messages to inbox"),
        Err(e) => {
            debug!(workspace = %workspace_id, error = %e, "inbox post failed; messages wait for a hook");
            let _ = store
                .update_workspace_quiet(workspace_id, |ws| {
                    ws.pending_messages.splice(0..0, messages);
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
                .is_some_and(|w| !w.pending_messages.is_empty())
        })
        .await;
    if !waiting {
        return None;
    }
    let messages = store
        .update_workspace_quiet(workspace_id, |ws| {
            Ok(if ws.agent == Agent::Claude {
                std::mem::take(&mut ws.pending_messages)
            } else {
                Vec::new()
            })
        })
        .await
        .ok()?;
    (!messages.is_empty()).then(|| messages.join("\n\n"))
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

#[cfg(test)]
mod tests {
    use super::*;

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

}
