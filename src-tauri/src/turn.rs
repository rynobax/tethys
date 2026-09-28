//! A session's turn state — the "your turn" indicator. Pure: persisting and
//! emitting are the supervisor's job.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

use crate::state::SessionRuntimeState;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TurnState {
    pub state: SessionRuntimeState,
    /// e.g. `permission_prompt`. Only ever `Some` while `WaitingInput`.
    pub notification_type: Option<String>,
    pub acknowledged: bool,
}

impl TurnState {
    pub fn needs_turn(&self, running: bool) -> bool {
        running
            && !self.acknowledged
            && matches!(
                self.state,
                SessionRuntimeState::Idle | SessionRuntimeState::WaitingInput
            )
    }

    pub fn is_working(&self, running: bool) -> bool {
        running && self.state == SessionRuntimeState::Working
    }
}

#[derive(Debug, Clone)]
pub enum TurnSignal {
    /// An *event*: a repeat of a dismissed state re-lights the indicator.
    Hook {
        state: SessionRuntimeState,
        notification_type: Option<String>,
    },
    /// A *poll* every 2s: corrects drift, but never re-lights a dismissed
    /// indicator.
    Probe { state: SessionRuntimeState },
    Spawned,
    /// May be mid-response.
    Reattached,
    Restored {
        state: SessionRuntimeState,
        notification_type: Option<String>,
        acknowledged: bool,
    },
    ChildExited,
    Acknowledged,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TurnChanged {
    pub workspace_id: String,
    pub session_id: String,
    pub runtime_state: SessionRuntimeState,
    pub notification_type: Option<String>,
    pub turn_acknowledged: bool,
}

#[derive(Default)]
pub struct TurnTracker {
    map: Mutex<HashMap<String, TurnState>>,
}

impl TurnTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Some` only for a change the user can see.
    pub fn observe(
        &self,
        session_id: &str,
        workspace_id: &str,
        signal: TurnSignal,
    ) -> Option<TurnChanged> {
        let mut map = self.map.lock().unwrap();
        // A never-seen session also defaults to Dormant; it hasn't exited.
        let known = map.contains_key(session_id);
        let current = map.entry(session_id.to_string()).or_default();
        let exited = known && current.state == SessionRuntimeState::Dormant;

        let changed = match signal {
            // Seeds tell nobody: `get_session` reads this map directly.
            TurnSignal::Spawned => {
                *current = TurnState {
                    state: SessionRuntimeState::WaitingInput,
                    notification_type: None,
                    acknowledged: false,
                };
                false
            }
            TurnSignal::Reattached => {
                *current = TurnState {
                    state: SessionRuntimeState::Working,
                    notification_type: None,
                    acknowledged: false,
                };
                false
            }
            TurnSignal::Restored {
                state,
                notification_type,
                acknowledged,
            } => {
                *current = TurnState {
                    state,
                    notification_type: normalize_subtype(state, notification_type),
                    acknowledged,
                };
                false
            }

            TurnSignal::ChildExited => {
                if exited {
                    false
                } else {
                    *current = TurnState {
                        state: SessionRuntimeState::Dormant,
                        notification_type: None,
                        acknowledged: false,
                    };
                    true
                }
            }

            TurnSignal::Hook {
                state,
                notification_type,
            } => {
                if exited {
                    false
                } else {
                    let nt = normalize_subtype(state, notification_type);
                    let unchanged = current.state == state && current.notification_type == nt;
                    if unchanged && !current.acknowledged {
                        false
                    } else {
                        *current = TurnState {
                            state,
                            notification_type: nt,
                            acknowledged: false,
                        };
                        true
                    }
                }
            }

            TurnSignal::Probe { state } => {
                if exited || current.state == state {
                    false
                } else {
                    // The probe can't see the subtype; keep the hooks' one.
                    let nt = normalize_subtype(state, current.notification_type.clone());
                    *current = TurnState {
                        state,
                        notification_type: nt,
                        acknowledged: false,
                    };
                    true
                }
            }

            TurnSignal::Acknowledged => {
                if current.acknowledged {
                    false
                } else {
                    current.acknowledged = true;
                    true
                }
            }
        };

        changed.then(|| TurnChanged {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            runtime_state: current.state,
            notification_type: current.notification_type.clone(),
            turn_acknowledged: current.acknowledged,
        })
    }

    pub fn get(&self, session_id: &str) -> TurnState {
        self.map
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }
}

fn normalize_subtype(
    state: SessionRuntimeState,
    notification_type: Option<String>,
) -> Option<String> {
    match state {
        SessionRuntimeState::WaitingInput => notification_type,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionRuntimeState::*;

    const WS: &str = "ws-1";
    const S: &str = "sess-1";

    fn hook(state: SessionRuntimeState, nt: Option<&str>) -> TurnSignal {
        TurnSignal::Hook {
            state,
            notification_type: nt.map(String::from),
        }
    }


    #[test]
    fn a_spawned_session_waits_for_input_and_emits_nothing() {
        let t = TurnTracker::new();
        assert_eq!(t.observe(S, WS, TurnSignal::Spawned), None);
        assert_eq!(t.get(S).state, WaitingInput);
    }

    #[test]
    fn a_reattached_session_is_assumed_mid_response() {
        let t = TurnTracker::new();
        assert_eq!(t.observe(S, WS, TurnSignal::Reattached), None);
        assert_eq!(t.get(S).state, Working);
    }

    /// Boot reattaches, then restores; the restore must win.
    #[test]
    fn restoring_from_disk_beats_the_reattach_seed() {
        let t = TurnTracker::new();
        t.observe(S, WS, TurnSignal::Reattached);
        t.observe(
            S,
            WS,
            TurnSignal::Restored {
                state: WaitingInput,
                notification_type: Some("permission_prompt".into()),
                acknowledged: false,
            },
        );

        let st = t.get(S);
        assert_eq!(st.state, WaitingInput);
        assert_eq!(st.notification_type.as_deref(), Some("permission_prompt"));
        assert!(st.needs_turn(true));
    }


    #[test]
    fn a_hook_transition_is_published() {
        let t = TurnTracker::new();
        t.observe(S, WS, TurnSignal::Spawned);
        let changed = t.observe(S, WS, hook(Working, None)).expect("published");
        assert_eq!(changed.runtime_state, Working);
        assert!(!changed.turn_acknowledged);
    }

    #[test]
    fn a_redundant_hook_is_not_republished() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Working, None));
        assert_eq!(t.observe(S, WS, hook(Working, None)), None);
    }

    #[test]
    fn a_repeated_hook_relights_an_acknowledged_indicator() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(WaitingInput, Some("idle_prompt")));
        t.observe(S, WS, TurnSignal::Acknowledged);
        assert!(!t.get(S).needs_turn(true));

        let changed = t
            .observe(S, WS, hook(WaitingInput, Some("idle_prompt")))
            .expect("re-lights");
        assert!(!changed.turn_acknowledged);
        assert!(t.get(S).needs_turn(true));
    }

    #[test]
    fn leaving_waiting_input_clears_the_notification_subtype() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(WaitingInput, Some("permission_request")));
        assert_eq!(
            t.get(S).notification_type.as_deref(),
            Some("permission_request")
        );

        t.observe(S, WS, hook(Working, None));
        assert_eq!(t.get(S).notification_type, None);
    }

    #[test]
    fn a_subtype_on_a_non_waiting_state_is_dropped() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Idle, Some("permission_prompt")));
        assert_eq!(t.get(S).notification_type, None);
    }


    #[test]
    fn a_probe_corrects_a_stale_hook_state() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Working, None));
        let changed = t
            .observe(S, WS, TurnSignal::Probe { state: Idle })
            .expect("published");
        assert_eq!(changed.runtime_state, Idle);
    }

    #[test]
    fn a_probe_does_not_relight_an_acknowledged_indicator() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Idle, None));
        t.observe(S, WS, TurnSignal::Acknowledged);

        for _ in 0..5 {
            assert_eq!(t.observe(S, WS, TurnSignal::Probe { state: Idle }), None);
        }
        assert!(!t.get(S).needs_turn(true), "stays dismissed");
    }

    #[test]
    fn a_probe_into_waiting_input_carries_no_subtype() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Working, None));

        let changed = t
            .observe(S, WS, TurnSignal::Probe { state: WaitingInput })
            .expect("published");
        assert_eq!(changed.notification_type, None);
    }

    #[test]
    fn a_probe_out_of_waiting_input_clears_the_subtype() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(WaitingInput, Some("permission_request")));

        let changed = t
            .observe(S, WS, TurnSignal::Probe { state: Working })
            .expect("published");
        assert_eq!(changed.notification_type, None);
        assert_eq!(t.get(S).notification_type, None);
    }


    #[test]
    fn child_exit_is_recorded_not_just_announced() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Idle, None));
        assert!(t.get(S).needs_turn(true));

        let changed = t
            .observe(S, WS, TurnSignal::ChildExited)
            .expect("published");
        assert_eq!(changed.runtime_state, Dormant);
        assert_eq!(t.get(S).state, Dormant, "the map actually changed");
        assert!(!t.get(S).needs_turn(false));
    }

    #[test]
    fn a_probe_never_resurrects_an_exited_session() {
        let t = TurnTracker::new();
        t.observe(S, WS, TurnSignal::ChildExited);
        assert_eq!(t.observe(S, WS, TurnSignal::Probe { state: Idle }), None);
        assert_eq!(t.get(S).state, Dormant);
    }

    #[test]
    fn a_late_hook_never_resurrects_an_exited_session() {
        let t = TurnTracker::new();
        t.observe(S, WS, TurnSignal::ChildExited);
        assert_eq!(t.observe(S, WS, hook(Working, None)), None);
        assert_eq!(t.get(S).state, Dormant);
    }

    #[test]
    fn exiting_twice_is_announced_once() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Working, None));
        assert!(t.observe(S, WS, TurnSignal::ChildExited).is_some());
        assert_eq!(t.observe(S, WS, TurnSignal::ChildExited), None);
    }


    #[test]
    fn acknowledging_twice_is_announced_once() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(Idle, None));
        assert!(t.observe(S, WS, TurnSignal::Acknowledged).is_some());
        assert_eq!(t.observe(S, WS, TurnSignal::Acknowledged), None);
    }


    #[test]
    fn a_dead_session_never_needs_a_turn() {
        let t = TurnTracker::new();
        t.observe(S, WS, hook(WaitingInput, Some("permission_prompt")));
        assert!(!t.get(S).needs_turn(false));
    }


    #[test]
    fn sessions_are_tracked_independently() {
        let t = TurnTracker::new();
        t.observe("a", WS, hook(Idle, None));
        t.observe("b", WS, hook(Working, None));
        assert!(t.get("a").needs_turn(true));
        assert!(!t.get("b").needs_turn(true));

        assert_eq!(t.get("b").state, Working);
        assert_eq!(t.get("never-seen"), TurnState::default());
    }
}
