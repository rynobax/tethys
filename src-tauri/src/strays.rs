//! Kills processes a session started that outlived the session — a dev server
//! backgrounded with `nohup`, a test run orphaned by a timed-out tool call.
//! They reparent to launchd, so no process tree leads back to the workspace,
//! but they keep the environment tmux gave the session: every process carrying
//! a session's tag belongs to it, and one whose session is gone is a stray.
//!
//! Sessions started before `TETHYS_SESSION_ID` existed carry only
//! `TETHYS_SPAWN_TOKEN`, so either tag counts, and a process is live if either
//! names a live session.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};
use tracing::{info, warn};

use crate::tmux;

pub const SESSION_ID_VAR: &str = "TETHYS_SESSION_ID";
const SPAWN_TOKEN_VAR: &str = "TETHYS_SPAWN_TOKEN";

const SWEEP_INTERVAL: Duration = Duration::from_secs(10 * 60);
const TERM_GRACE: Duration = Duration::from_secs(10);

/// A dedicated thread: a sweep is blocking syscalls and a grace-period sleep.
pub fn spawn(tmux_bin: PathBuf) {
    std::thread::spawn(move || {
        let own = Tags::of_env(std::env::vars_os().map(|(k, v)| {
            let mut kv = k;
            kv.push("=");
            kv.push(v);
            kv
        }));
        loop {
            sweep(&tmux_bin, &own);
            std::thread::sleep(SWEEP_INTERVAL);
        }
    });
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Tags {
    session_id: Option<String>,
    spawn_token: Option<String>,
}

impl Tags {
    fn of_env(env: impl IntoIterator<Item = OsString>) -> Self {
        let mut tags = Tags::default();
        for kv in env {
            let Some(kv) = kv.to_str() else { continue };
            let Some((key, value)) = kv.split_once('=') else { continue };
            match key {
                SESSION_ID_VAR => tags.session_id = Some(value.to_string()),
                SPAWN_TOKEN_VAR => tags.spawn_token = Some(value.to_string()),
                _ => {}
            }
        }
        tags
    }

    fn is_empty(&self) -> bool {
        self.session_id.is_none() && self.spawn_token.is_none()
    }

    fn shares_any(&self, other: &Tags) -> bool {
        (self.session_id.is_some() && self.session_id == other.session_id)
            || (self.spawn_token.is_some() && self.spawn_token == other.spawn_token)
    }
}

#[derive(Debug, Default)]
struct Live {
    session_ids: HashSet<String>,
    spawn_tokens: HashSet<String>,
}

impl Live {
    fn owns(&self, tags: &Tags) -> bool {
        tags.session_id
            .as_ref()
            .is_some_and(|id| self.session_ids.contains(id))
            || tags
                .spawn_token
                .as_ref()
                .is_some_and(|t| self.spawn_tokens.contains(t))
    }
}

/// `None` if tmux can't vouch for every session: a session missing from the
/// answer would read as dead, and its processes as strays.
fn live_sessions(tmux_bin: &Path) -> Option<Live> {
    let mut live = Live::default();
    for id in tmux::try_list_sessions(tmux_bin)? {
        let env = tmux::session_environment(tmux_bin, &id)?;
        if let Some(token) = Tags::of_env(env.into_iter().map(OsString::from)).spawn_token {
            live.spawn_tokens.insert(token);
        }
        live.session_ids.insert(id);
    }
    Some(live)
}

struct Candidate {
    pid: Pid,
    parent: Option<Pid>,
    start_time: u64,
    tags: Tags,
    /// tmux's server and memwatch both outlive the Tethys that started them,
    /// so they can carry a tag from a Tethys run inside a since-dead session.
    guarded: bool,
    label: String,
    rss_mb: u64,
}

/// A stray carries a tag no live session owns. Never Tethys or anything above
/// it, nor anything under a guarded process, nor anything sharing Tethys's own
/// tags — `pnpm tauri dev` run from inside a session puts the whole dev tree,
/// Tethys included, under that session's tag.
fn find_strays<'a>(
    candidates: &'a [Candidate],
    live: &Live,
    own: &Tags,
    own_pid: Pid,
) -> Vec<&'a Candidate> {
    let by_pid: std::collections::HashMap<Pid, &Candidate> =
        candidates.iter().map(|c| (c.pid, c)).collect();
    let ancestors = |pid: Pid| {
        std::iter::successors(by_pid.get(&pid).and_then(|c| c.parent), |p| {
            by_pid.get(p).and_then(|c| c.parent)
        })
        // A hand-edited or racing table could loop; no real chain is this deep.
        .take(64)
    };
    let above_us: HashSet<Pid> = std::iter::once(own_pid).chain(ancestors(own_pid)).collect();
    let under_guard = |c: &Candidate| {
        c.guarded
            || ancestors(c.pid).any(|p| p == own_pid || by_pid.get(&p).is_some_and(|a| a.guarded))
    };

    candidates
        .iter()
        .filter(|c| !c.tags.is_empty())
        .filter(|c| !live.owns(&c.tags))
        .filter(|c| !c.tags.shares_any(own))
        .filter(|c| !above_us.contains(&c.pid))
        .filter(|c| !under_guard(c))
        .collect()
}

fn snapshot() -> System {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_memory()
            .with_cmd(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    sys
}

fn candidates(sys: &System) -> Vec<Candidate> {
    sys.processes()
        .values()
        .map(|p| {
            let name = p.name().to_string_lossy();
            let cmd = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            let guarded = name == "tmux" || cmd.contains("memwatch.sh");
            Candidate {
                pid: p.pid(),
                parent: p.parent(),
                start_time: p.start_time(),
                tags: Tags::of_env(p.environ().iter().cloned()),
                guarded,
                label: cmd.chars().take(160).collect(),
                rss_mb: p.memory() / (1024 * 1024),
            }
        })
        .collect()
}

fn sweep(tmux_bin: &Path, own: &Tags) {
    // Processes before sessions: a session started in between has no
    // processes in this snapshot, so its absence from `live` can't hurt it.
    let sys = snapshot();
    let Some(live) = live_sessions(tmux_bin) else {
        warn!("stray sweep skipped: tmux couldn't list its sessions");
        return;
    };
    let all = candidates(&sys);
    let own_pid = Pid::from_u32(std::process::id());
    let strays = find_strays(&all, &live, own, own_pid);
    if strays.is_empty() {
        return;
    }

    let targets: Vec<(Pid, u64)> = strays.iter().map(|c| (c.pid, c.start_time)).collect();
    for c in &strays {
        info!(
            pid = %c.pid,
            rss_mb = c.rss_mb,
            session_id = ?c.tags.session_id,
            spawn_token = ?c.tags.spawn_token,
            command = %c.label,
            "killing stray process whose session is gone"
        );
    }
    signal_if_same(&targets, Signal::Term);
    std::thread::sleep(TERM_GRACE);
    let survivors = signal_if_same(&targets, Signal::Kill);
    if survivors > 0 {
        warn!(count = survivors, "stray processes ignored SIGTERM; sent SIGKILL");
    }
}

/// Re-reads each pid first, so a pid the kernel has since reused for an
/// unrelated process is left alone. Returns how many were signalled.
fn signal_if_same(targets: &[(Pid, u64)], signal: Signal) -> usize {
    let pids: Vec<Pid> = targets.iter().map(|(p, _)| *p).collect();
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        true,
        ProcessRefreshKind::nothing(),
    );
    targets
        .iter()
        .filter_map(|(pid, start)| sys.process(*pid).filter(|p| p.start_time() == *start))
        .filter(|p| p.kill_with(signal).unwrap_or(false))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(session_id: Option<&str>, spawn_token: Option<&str>) -> Tags {
        Tags {
            session_id: session_id.map(String::from),
            spawn_token: spawn_token.map(String::from),
        }
    }

    fn proc(pid: u32, parent: u32, tags: Tags) -> Candidate {
        Candidate {
            pid: Pid::from_u32(pid),
            parent: Some(Pid::from_u32(parent)),
            start_time: 0,
            tags,
            guarded: false,
            label: String::new(),
            rss_mb: 0,
        }
    }

    fn live(session_ids: &[&str], spawn_tokens: &[&str]) -> Live {
        Live {
            session_ids: session_ids.iter().map(|s| s.to_string()).collect(),
            spawn_tokens: spawn_tokens.iter().map(|s| s.to_string()).collect(),
        }
    }

    const TETHYS: u32 = 500;

    fn stray_pids(candidates: &[Candidate], live: &Live, own: &Tags) -> Vec<u32> {
        let mut pids: Vec<u32> = find_strays(candidates, live, own, Pid::from_u32(TETHYS))
            .iter()
            .map(|c| c.pid.as_u32())
            .collect();
        pids.sort();
        pids
    }

    #[test]
    fn reads_both_tags_from_an_environment() {
        let env = ["PATH=/bin", "TETHYS_SESSION_ID=s1", "TETHYS_SPAWN_TOKEN=t1", "NOEQUALS"]
            .map(OsString::from);
        assert_eq!(Tags::of_env(env), tags(Some("s1"), Some("t1")));
    }

    #[test]
    fn a_process_whose_session_is_gone_is_a_stray() {
        let procs = [proc(10, 1, tags(Some("dead"), Some("dead-tok")))];
        assert_eq!(stray_pids(&procs, &live(&["alive"], &["alive-tok"]), &Tags::default()), [10]);
    }

    #[test]
    fn a_process_of_a_live_session_is_left_alone() {
        let procs = [
            proc(10, 1, tags(Some("alive"), None)),
            proc(11, 1, tags(None, Some("alive-tok"))),
        ];
        assert!(stray_pids(&procs, &live(&["alive"], &["alive-tok"]), &Tags::default()).is_empty());
    }

    /// Codex may filter `*TOKEN*` vars out of the commands it runs.
    #[test]
    fn either_tag_naming_a_live_session_is_enough() {
        let procs = [proc(10, 1, tags(Some("alive"), Some("stale-tok")))];
        assert!(stray_pids(&procs, &live(&["alive"], &[]), &Tags::default()).is_empty());
    }

    #[test]
    fn untagged_processes_are_never_touched() {
        let procs = [proc(10, 1, Tags::default())];
        assert!(stray_pids(&procs, &live(&[], &[]), &Tags::default()).is_empty());
    }

    #[test]
    fn tethys_its_ancestors_and_its_dev_tree_are_spared() {
        let dev_session = tags(None, Some("dev-tok"));
        let procs = [
            proc(400, 1, dev_session.clone()),    // pnpm tauri dev
            proc(TETHYS, 400, dev_session.clone()),
            proc(401, 400, dev_session.clone()), // vite, a sibling
            proc(600, TETHYS, tags(None, Some("dead"))),
        ];
        assert!(stray_pids(&procs, &live(&[], &[]), &dev_session).is_empty());
    }

    #[test]
    fn a_guarded_process_and_its_children_are_spared() {
        let mut tmux_server = proc(20, 1, tags(None, Some("old-run")));
        tmux_server.guarded = true;
        let procs = [tmux_server, proc(21, 20, tags(None, Some("old-run")))];
        assert!(stray_pids(&procs, &live(&[], &[]), &Tags::default()).is_empty());
    }

    #[test]
    fn a_strays_children_are_strays_too() {
        let dead = tags(None, Some("dead"));
        let procs = [proc(30, 1, dead.clone()), proc(31, 30, dead.clone()), proc(32, 30, dead)];
        assert_eq!(stray_pids(&procs, &live(&[], &[]), &Tags::default()), [30, 31, 32]);
    }

    #[test]
    fn a_parent_cycle_does_not_hang_the_walk() {
        let procs = [proc(40, 41, tags(None, Some("dead"))), proc(41, 40, tags(None, Some("dead")))];
        assert_eq!(stray_pids(&procs, &live(&[], &[]), &Tags::default()), [40, 41]);
    }
}
