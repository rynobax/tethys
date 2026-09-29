//! Closes a session nobody has touched in a day. Closing kills the tmux
//! session and nothing else: the conversation stays on disk, and Resume picks
//! it back up.
//!
//! "Touched" is the user typing into it or the agent firing a hook, so a
//! session still working through a long turn isn't idle. A session sitting in
//! `Working` is never closed either — a tool call can run for hours without a
//! hook, and killing one mid-flight loses more than it frees.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use tracing::{info, warn};

use crate::sessions::{SessionId, SessionSupervisor};
use crate::state::{AppState, SessionRuntimeState, WorkspaceId};
use crate::store::Store;
use crate::tmux;

const IDLE_LIMIT: TimeDelta = TimeDelta::hours(24);
const SWEEP_INTERVAL: Duration = Duration::from_secs(10 * 60);

pub fn spawn(supervisor: Arc<SessionSupervisor>, store: Arc<Store>, tmux_bin: PathBuf) {
    tauri::async_runtime::spawn(async move {
        loop {
            sweep(&supervisor, &store, &tmux_bin).await;
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}

async fn sweep(supervisor: &SessionSupervisor, store: &Store, tmux_bin: &std::path::Path) {
    let touched = supervisor.take_activity();
    let now = Utc::now();
    let due = store
        .mutate(|s| Ok(record_activity_and_find_idle(s, &touched, now)))
        .await;
    let due = match due {
        Ok(due) => due,
        Err(e) => {
            warn!(error = %e, "idle sweep failed");
            return;
        }
    };

    for (workspace_id, session_id) in due {
        if !tmux::has_session(tmux_bin, &session_id) {
            continue;
        }
        info!(%workspace_id, %session_id, "closing session idle for over 24h");
        tmux::kill_session(tmux_bin, &session_id);
    }
}

/// A session with no `last_active_at` predates the field; its clock starts now.
fn record_activity_and_find_idle(
    state: &mut AppState,
    touched: &HashMap<SessionId, DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Vec<(WorkspaceId, SessionId)> {
    let mut due = Vec::new();
    for ws in state.workspaces.iter_mut().filter(|w| w.deleted_at.is_none()) {
        let Some(meta) = ws.session.as_mut() else { continue };
        let last = match (meta.last_active_at, touched.get(&meta.id)) {
            (Some(stored), Some(&t)) => stored.max(t),
            (stored, t) => stored.or(t.copied()).unwrap_or(now),
        };
        meta.last_active_at = Some(last);
        if meta.runtime_state != Some(SessionRuntimeState::Working) && now - last >= IDLE_LIMIT {
            due.push((ws.id.clone(), meta.id.clone()));
        }
    }
    due
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::state::{AgentSessionMeta, Origin, Workspace};

    fn state_with(meta: AgentSessionMeta) -> AppState {
        let mut ws = Workspace::draft(
            "ws".into(),
            "b".into(),
            Agent::Claude,
            None,
            Origin::Ui,
            None,
        );
        ws.session = Some(meta);
        AppState {
            workspaces: vec![ws],
            ..AppState::default()
        }
    }

    fn meta(
        last_active_at: Option<DateTime<Utc>>,
        runtime_state: SessionRuntimeState,
    ) -> AgentSessionMeta {
        AgentSessionMeta {
            id: "sess".into(),
            cwd: PathBuf::from("/wt/ws"),
            agent_session_id: None,
            transcript_path: None,
            runtime_state: Some(runtime_state),
            notification_type: None,
            turn_acknowledged: false,
            last_active_at,
        }
    }

    fn now() -> DateTime<Utc> {
        "2026-09-29T12:00:00Z".parse().unwrap()
    }

    fn last_active(state: &AppState) -> Option<DateTime<Utc>> {
        state.workspaces[0].session.as_ref().unwrap().last_active_at
    }

    #[test]
    fn a_session_untouched_for_a_day_is_due() {
        let mut s = state_with(meta(Some(now() - IDLE_LIMIT), SessionRuntimeState::Idle));
        let due = record_activity_and_find_idle(&mut s, &HashMap::new(), now());
        assert_eq!(due, vec![("ws".to_string(), "sess".to_string())]);
    }

    #[test]
    fn a_session_touched_within_the_day_is_not() {
        let mut s = state_with(meta(
            Some(now() - IDLE_LIMIT + TimeDelta::minutes(1)),
            SessionRuntimeState::WaitingInput,
        ));
        assert!(record_activity_and_find_idle(&mut s, &HashMap::new(), now()).is_empty());
    }

    #[test]
    fn fresh_activity_saves_a_stale_session_and_is_persisted() {
        let mut s = state_with(meta(Some(now() - TimeDelta::days(3)), SessionRuntimeState::Idle));
        let recent = now() - TimeDelta::hours(1);
        let touched = HashMap::from([("sess".to_string(), recent)]);

        assert!(record_activity_and_find_idle(&mut s, &touched, now()).is_empty());
        assert_eq!(last_active(&s), Some(recent));
    }

    #[test]
    fn a_working_session_is_never_due() {
        let mut s = state_with(meta(Some(now() - TimeDelta::days(3)), SessionRuntimeState::Working));
        assert!(record_activity_and_find_idle(&mut s, &HashMap::new(), now()).is_empty());
    }

    #[test]
    fn a_session_from_before_tracking_starts_its_clock_now() {
        let mut s = state_with(meta(None, SessionRuntimeState::Idle));
        assert!(record_activity_and_find_idle(&mut s, &HashMap::new(), now()).is_empty());
        assert_eq!(last_active(&s), Some(now()));
    }

    #[test]
    fn a_deleted_workspace_is_skipped() {
        let mut s = state_with(meta(Some(now() - TimeDelta::days(3)), SessionRuntimeState::Idle));
        s.workspaces[0].deleted_at = Some(now());
        assert!(record_activity_and_find_idle(&mut s, &HashMap::new(), now()).is_empty());
    }
}
