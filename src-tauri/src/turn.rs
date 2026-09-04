//! Ownership of a session's turn state — the "your turn" indicator.
//!
//! Seven signals can change what a session is showing: Claude Code hooks,
//! Claude's own status probe files, a fresh spawn, a reattach, a restore from
//! `state.json` at boot, the PTY child exiting, and the user dismissing the
//! indicator. Before this module each of those wrote the state map itself, and
//! the rules for how they interact lived in whichever call site happened to
//! need them — six methods on the supervisor, a loop in the probe reconciler,
//! an override at boot, one command, and four predicates in the frontend.
//!
//! Everything here is pure: an in-memory map and a `match`. Persisting and
//! emitting are the supervisor's job, so this stays testable without a store,
//! an `AppHandle`, or a tokio runtime.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

use crate::state::SessionRuntimeState;

/// Per-session turn state. The persisted mirror lives on `ClaudeSessionMeta`
/// so all three fields survive a restart.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TurnState {
    pub state: SessionRuntimeState,
    /// Subtype of the last `WaitingInput` transition (e.g. `permission_prompt`),
    /// so the UI can render a permission prompt more urgently than an idle one.
    /// Only ever `Some` while `state` is `WaitingInput` — enforced here rather
    /// than by each caller remembering to pass `None`.
    pub notification_type: Option<String>,
    /// The user dismissed the indicator. Cleared by the next fresh signal.
    pub acknowledged: bool,
}

impl TurnState {
    /// Whether this session wants the user's attention.
    ///
    /// Takes `running` because a session whose PTY is gone must never nag,
    /// even if the last thing a hook said was "waiting for input". The
    /// frontend used to compute this in four places, two of which disagreed
    /// about exactly this condition.
    pub fn needs_turn(&self, running: bool) -> bool {
        running
            && !self.acknowledged
            && matches!(
                self.state,
                SessionRuntimeState::Idle | SessionRuntimeState::WaitingInput
            )
    }

    /// Whether this session is actively working.
    pub fn is_working(&self, running: bool) -> bool {
        running && self.state == SessionRuntimeState::Working
    }
}

/// Something that can change a session's turn state. One variant per source,
/// so the precedence between sources is a `match` in one place instead of an
/// ordering property of code spread across five files.
#[derive(Debug, Clone)]
pub enum TurnSignal {
    /// A Claude Code hook. An *event*: a repeated signal for a state the user
    /// already dismissed is a fresh nudge and re-lights the indicator.
    Hook {
        state: SessionRuntimeState,
        notification_type: Option<String>,
    },
    /// Claude's own status probe file, polled every 2s. A *poll*, not an
    /// event: it reports drift the hook stream missed, but must never
    /// re-light an indicator the user just cleared — otherwise dismissing a
    /// turn would last at most two seconds.
    Probe { state: SessionRuntimeState },
    /// A freshly spawned (or resumed) session, sitting at an empty prompt.
    Spawned,
    /// Reattached to a tmux pane that outlived the app; may be mid-response.
    Reattached,
    /// Restored from `state.json` at boot.
    Restored {
        state: SessionRuntimeState,
        notification_type: Option<String>,
        acknowledged: bool,
    },
    /// The PTY child exited.
    ChildExited,
    /// The user dismissed the indicator.
    Acknowledged,
}

/// What the UI needs to be told, and what gets written through to
/// `state.json`. Returned only when something actually changed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TurnChanged {
    pub workspace_id: String,
    pub session_id: String,
    pub runtime_state: SessionRuntimeState,
    pub notification_type: Option<String>,
    pub turn_acknowledged: bool,
}

/// The turn state of every session Tethys is tracking.
#[derive(Default)]
pub struct TurnTracker {
    map: Mutex<HashMap<String, TurnState>>,
}

impl TurnTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a signal.
    ///
    /// `Some` means the state changed in a way the user can see: emit
    /// `session:turn_changed` and persist. `None` means nothing to do — a
    /// redundant signal, a seed that established initial state, or a signal
    /// for a session that has already exited.
    pub fn observe(
        &self,
        session_id: &str,
        workspace_id: &str,
        signal: TurnSignal,
    ) -> Option<TurnChanged> {
        let mut map = self.map.lock().unwrap();
        // Distinguish "we have never seen this session" from "this session
        // exited". Both would read as `Dormant` if we defaulted the entry in
        // here, and the exit guard below would then swallow the first hook of
        // every new session.
        let known = map.contains_key(session_id);
        let current = map.entry(session_id.to_string()).or_default();
        let exited = known && current.state == SessionRuntimeState::Dormant;

        let changed = match signal {
            // ── Seeds: establish initial state, tell nobody ────────────────
            // The frontend isn't subscribed yet at boot, and `get_session`
            // reads the value straight out of this map anyway.
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

            // ── The PTY is gone ───────────────────────────────────────────
            // Terminal. A dead session shows Dormant regardless of what the
            // last hook claimed, and nothing below can bring it back.
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

            // ── Live signals ──────────────────────────────────────────────
            TurnSignal::Hook {
                state,
                notification_type,
            } => {
                if exited {
                    // A late hook from an exited session must not resurrect it.
                    false
                } else {
                    let nt = normalize_subtype(state, notification_type);
                    let unchanged = current.state == state && current.notification_type == nt;
                    // An acknowledged indicator re-lights on the next hook even
                    // when the state is identical: a repeated idle_prompt is a
                    // fresh nudge, not noise.
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
                    // Same state: nothing drifted. Crucially this does NOT
                    // re-light an acknowledged indicator — the probe fires
                    // every 2s, so treating it as a fresh nudge would make
                    // dismissing a turn meaningless.
                    false
                } else {
                    // The probe can't tell a permission prompt from an idle
                    // one, so keep whatever subtype the hooks established if
                    // we're still waiting on input.
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

    /// This session's current turn state (default if never seen).
    pub fn get(&self, session_id: &str) -> TurnState {
        self.map
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

}

/// The notification subtype only means anything while waiting on input.
/// Enforced in one place so leaving `WaitingInput` can't strand a stale
/// subtype — previously a documented invariant that six call sites had to
/// remember, and that the probe reconciler re-implemented by hand.
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

    fn tracker() -> TurnTracker {
        TurnTracker::new()
    }

    fn hook(state: SessionRuntimeState, nt: Option<&str>) -> TurnSignal {
        TurnSignal::Hook {
            state,
            notification_type: nt.map(String::from),
        }
    }

    // ── seeds ────────────────────────────────────────────────────────────

    /// A fresh spawn lands at an empty prompt waiting on the user — "your
    /// turn", not "working". Seeds tell nobody: the frontend reads the value
    /// out of `get_session` instead.
    #[test]
    fn a_spawned_session_waits_for_input_and_emits_nothing() {
        let t = tracker();
        assert_eq!(t.observe(S, WS, TurnSignal::Spawned), None);
        assert_eq!(t.get(S).state, WaitingInput);
    }

    #[test]
    fn a_reattached_session_is_assumed_mid_response() {
        let t = tracker();
        assert_eq!(t.observe(S, WS, TurnSignal::Reattached), None);
        assert_eq!(t.get(S).state, Working);
    }

    /// At boot the supervisor reattaches (seeding Working) and then restores
    /// from disk. The restore has to win, or every session comes back showing
    /// "working" and the sidebar dot stays dark across restarts.
    #[test]
    fn restoring_from_disk_beats_the_reattach_seed() {
        let t = tracker();
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

    // ── hooks ────────────────────────────────────────────────────────────

    #[test]
    fn a_hook_transition_is_published() {
        let t = tracker();
        t.observe(S, WS, TurnSignal::Spawned);
        let changed = t.observe(S, WS, hook(Working, None)).expect("published");
        assert_eq!(changed.runtime_state, Working);
        assert!(!changed.turn_acknowledged);
    }

    #[test]
    fn a_redundant_hook_is_not_republished() {
        let t = tracker();
        t.observe(S, WS, hook(Working, None));
        assert_eq!(t.observe(S, WS, hook(Working, None)), None);
    }

    /// A repeated idle_prompt is Claude nudging again, not noise — a cleared
    /// row must not stay dark forever.
    #[test]
    fn a_repeated_hook_relights_an_acknowledged_indicator() {
        let t = tracker();
        t.observe(S, WS, hook(WaitingInput, Some("idle_prompt")));
        t.observe(S, WS, TurnSignal::Acknowledged);
        assert!(!t.get(S).needs_turn(true));

        let changed = t
            .observe(S, WS, hook(WaitingInput, Some("idle_prompt")))
            .expect("re-lights");
        assert!(!changed.turn_acknowledged);
        assert!(t.get(S).needs_turn(true));
    }

    /// The subtype is only meaningful while waiting on input. This was a
    /// documented invariant that every caller had to uphold by hand.
    #[test]
    fn leaving_waiting_input_clears_the_notification_subtype() {
        let t = tracker();
        t.observe(S, WS, hook(WaitingInput, Some("permission_request")));
        assert_eq!(
            t.get(S).notification_type.as_deref(),
            Some("permission_request")
        );

        t.observe(S, WS, hook(Working, None));
        assert_eq!(t.get(S).notification_type, None);
    }

    /// Even if a caller passes one, a non-WaitingInput state can't carry a
    /// subtype.
    #[test]
    fn a_subtype_on_a_non_waiting_state_is_dropped() {
        let t = tracker();
        t.observe(S, WS, hook(Idle, Some("permission_prompt")));
        assert_eq!(t.get(S).notification_type, None);
    }

    // ── probes ───────────────────────────────────────────────────────────

    /// The probe corrects drift the hook stream missed.
    #[test]
    fn a_probe_corrects_a_stale_hook_state() {
        let t = tracker();
        t.observe(S, WS, hook(Working, None));
        let changed = t
            .observe(S, WS, TurnSignal::Probe { state: Idle })
            .expect("published");
        assert_eq!(changed.runtime_state, Idle);
    }

    /// The regression that matters most: the probe fires every 2s, so if it
    /// re-lit an acknowledged indicator the way a hook does, dismissing a turn
    /// would last at most two seconds.
    #[test]
    fn a_probe_does_not_relight_an_acknowledged_indicator() {
        let t = tracker();
        t.observe(S, WS, hook(Idle, None));
        t.observe(S, WS, TurnSignal::Acknowledged);

        for _ in 0..5 {
            assert_eq!(t.observe(S, WS, TurnSignal::Probe { state: Idle }), None);
        }
        assert!(!t.get(S).needs_turn(true), "stays dismissed");
    }

    /// A probe can't tell a permission prompt from an idle one, so it never
    /// invents a subtype: moving into WaitingInput on probe evidence alone
    /// carries none, and the hooks fill it in if there is one.
    #[test]
    fn a_probe_into_waiting_input_carries_no_subtype() {
        let t = tracker();
        t.observe(S, WS, hook(Working, None));

        let changed = t
            .observe(S, WS, TurnSignal::Probe { state: WaitingInput })
            .expect("published");
        assert_eq!(changed.notification_type, None);
    }

    /// A probe moving a session out of WaitingInput clears the subtype, same
    /// as a hook would. The probe reconciler used to re-implement this rule by
    /// hand, separately from the one on the hook path.
    #[test]
    fn a_probe_out_of_waiting_input_clears_the_subtype() {
        let t = tracker();
        t.observe(S, WS, hook(WaitingInput, Some("permission_request")));

        let changed = t
            .observe(S, WS, TurnSignal::Probe { state: Working })
            .expect("published");
        assert_eq!(changed.notification_type, None);
        assert_eq!(t.get(S).notification_type, None);
    }

    // ── exit ─────────────────────────────────────────────────────────────

    /// The bug this module was written for: the exit hook used to emit a
    /// Dormant event without writing it anywhere, so `get_session` kept
    /// reporting the pre-exit state and a refresh re-lit the dot for a dead
    /// session.
    #[test]
    fn child_exit_is_recorded_not_just_announced() {
        let t = tracker();
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
        let t = tracker();
        t.observe(S, WS, TurnSignal::ChildExited);
        assert_eq!(t.observe(S, WS, TurnSignal::Probe { state: Idle }), None);
        assert_eq!(t.get(S).state, Dormant);
    }

    #[test]
    fn a_late_hook_never_resurrects_an_exited_session() {
        let t = tracker();
        t.observe(S, WS, TurnSignal::ChildExited);
        assert_eq!(t.observe(S, WS, hook(Working, None)), None);
        assert_eq!(t.get(S).state, Dormant);
    }

    #[test]
    fn exiting_twice_is_announced_once() {
        let t = tracker();
        t.observe(S, WS, hook(Working, None));
        assert!(t.observe(S, WS, TurnSignal::ChildExited).is_some());
        assert_eq!(t.observe(S, WS, TurnSignal::ChildExited), None);
    }

    // ── acknowledgement ──────────────────────────────────────────────────

    #[test]
    fn acknowledging_twice_is_announced_once() {
        let t = tracker();
        t.observe(S, WS, hook(Idle, None));
        assert!(t.observe(S, WS, TurnSignal::Acknowledged).is_some());
        assert_eq!(t.observe(S, WS, TurnSignal::Acknowledged), None);
    }

    // ── needs_turn ───────────────────────────────────────────────────────

    /// The predicate the frontend used to compute in four places, two of which
    /// disagreed about whether `running` mattered.
    #[test]
    fn needs_turn_over_the_whole_state_space() {
        for state in [Dormant, Working, Idle, WaitingInput] {
            for running in [true, false] {
                for acknowledged in [true, false] {
                    let st = TurnState {
                        state,
                        notification_type: None,
                        acknowledged,
                    };
                    let expected = running
                        && !acknowledged
                        && matches!(state, Idle | WaitingInput);
                    assert_eq!(
                        st.needs_turn(running),
                        expected,
                        "state={state:?} running={running} acked={acknowledged}"
                    );
                }
            }
        }
    }

    /// A session whose PTY died must never nag, whatever the last hook said.
    #[test]
    fn a_dead_session_never_needs_a_turn() {
        let t = tracker();
        t.observe(S, WS, hook(WaitingInput, Some("permission_prompt")));
        assert!(!t.get(S).needs_turn(false));
    }

    // ── isolation ────────────────────────────────────────────────────────

    #[test]
    fn sessions_are_tracked_independently() {
        let t = tracker();
        t.observe("a", WS, hook(Idle, None));
        t.observe("b", WS, hook(Working, None));
        assert!(t.get("a").needs_turn(true));
        assert!(!t.get("b").needs_turn(true));

        assert_eq!(t.get("b").state, Working);
        assert_eq!(t.get("never-seen"), TurnState::default());
    }
}
