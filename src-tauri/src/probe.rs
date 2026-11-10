//! Backstop for hook-derived turn state, from the `status` Claude writes to
//! `~/.claude/sessions/<pid>.json`. It stays right through in-process
//! subagents and dropped hooks, but can't tell a permission prompt from an idle
//! one, so hooks keep the `notification_type`.
//!
//! `statusUpdatedAt` only moves on transitions, so it can't detect a stuck
//! session.
//!
//! Claude-only: codex session ids never rotate.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tracing::debug;

use crate::sessions::SessionSupervisor;
use crate::state::SessionRuntimeState;

/// Optional everywhere so a partial write or schema shift still parses.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Probe {
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub status: Option<String>,
    /// Survives the session-id rotation on compaction/resume, so it's the
    /// fallback correlation key.
    pub cwd: Option<String>,
    /// Epoch ms. Orders two probes for one cwd, telling the live session from
    /// the ghost file a rotation leaves behind.
    #[serde(rename = "statusUpdatedAt")]
    pub status_updated_at: Option<i64>,
}

fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".claude").join("sessions"))
}

/// `None` defers to the hooks. `shell` is a turn that ended with a background
/// shell still running; the agent wakes when it finishes, so it isn't Ryan's
/// turn yet. A backgrounded dev server never finishes and so reads as working
/// for as long as it runs.
pub fn state_from_status(status: &str) -> Option<SessionRuntimeState> {
    match status {
        "busy" | "shell" => Some(SessionRuntimeState::Working),
        "waiting" => Some(SessionRuntimeState::WaitingInput),
        "idle" => Some(SessionRuntimeState::Idle),
        _ => None,
    }
}

async fn read_all() -> Vec<Probe> {
    let Some(dir) = sessions_dir() else {
        return Vec::new();
    };
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(&path).await else {
            continue;
        };
        match serde_json::from_slice::<Probe>(&bytes) {
            Ok(p) => out.push(p),
            Err(e) => {
                debug!(path = %path.display(), error = %e, "probe parse failed")
            }
        }
    }
    out
}

const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub fn spawn(supervisor: Arc<SessionSupervisor>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let probes = read_all().await;
            supervisor.reconcile_probes(&probes).await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::state_from_status;
    use crate::state::SessionRuntimeState;

    #[test]
    fn a_background_shell_is_still_working() {
        assert_eq!(state_from_status("shell"), Some(SessionRuntimeState::Working));
    }

}
