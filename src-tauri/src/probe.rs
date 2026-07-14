//! Reads Claude Code's per-session status probe files
//! (`~/.claude/sessions/<pid>.json`) and reconciles them against the
//! hook-derived turn state.
//!
//! Claude writes an authoritative `status` field (`busy` / `shell` / `idle`
//! / `waiting`) to one file per live session. Unlike the hook stream, that
//! field stays correct while a subagent runs in-process (the parent session
//! remains `busy`) and survives a dropped hook — so it's used as a backstop
//! that corrects drift the hooks can't see. It's coarser than hooks, though:
//! it can't tell a permission prompt from an idle prompt, so hooks still own
//! the `notification_type` subtype.
//!
//! `statusUpdatedAt` is written on state *transitions only*, not as a
//! heartbeat, so it can't be used to infer a stuck session — an hour-old
//! `idle` stamp just means the session went idle an hour ago.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tracing::debug;

use crate::sessions::SessionSupervisor;
use crate::state::SessionRuntimeState;

/// Subset of a `~/.claude/sessions/<pid>.json` probe file. Every field is
/// optional so a partial write or schema shift never breaks parsing.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Probe {
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub status: Option<String>,
    /// Session working directory. Stable across the session-id rotation
    /// Claude does on compaction/resume, so it's the correlation key we fall
    /// back to when the stored `claude_session_id` has gone stale.
    pub cwd: Option<String>,
}

/// `~/.claude/sessions/` — where Claude Code drops one probe file per live
/// session.
fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".claude").join("sessions"))
}

/// Map Claude's raw probe status onto a Tethys runtime state. Returns `None`
/// for anything unrecognised, so a novel status value never clobbers a good
/// hook-derived state. `shell` (Claude running a bang-command) counts as
/// working.
pub fn state_from_status(status: &str) -> Option<SessionRuntimeState> {
    match status {
        "busy" | "shell" => Some(SessionRuntimeState::Working),
        "waiting" => Some(SessionRuntimeState::WaitingInput),
        "idle" => Some(SessionRuntimeState::Idle),
        _ => None,
    }
}

/// Read and parse every probe file in `~/.claude/sessions/`. Bad or partial
/// files are skipped, never fatal.
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

/// Spawn the probe reconciliation loop for the life of the app. Every couple
/// seconds it reads all probe files and hands them to the supervisor, which
/// correlates each by `sessionId` and corrects any session whose hook-derived
/// state has drifted from Claude's own.
pub fn spawn(supervisor: Arc<SessionSupervisor>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let probes = read_all().await;
            supervisor.reconcile_probes(&probes).await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}
