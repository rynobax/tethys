//! Kills the biggest job a session is running once everything it started
//! holds more memory than its budget. macOS enforces no memory limit on a
//! process or a tree — `ulimit -v` is refused, `RLIMIT_RSS` ignored — so a
//! runaway test run doesn't fail, it swaps the whole machine to a standstill.
//!
//! A session's usage is the summed footprint of every process carrying its
//! `TETHYS_SESSION_ID`, minus the agent CLI, which is never counted or killed.
//! A job is a subtree hanging off the agent (a tool call, a background task,
//! an MCP server) or one that detached from it.
//!
//! An agent that needs more can ask, over MCP, for a higher budget for a
//! while. Only the user can grant it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::sessions::SessionSupervisor;
use crate::state::{SessionId, WorkspaceId};
use crate::strays::{self, Tags};

const DEFAULT_BUDGET_MB: u64 = 8 * 1024;
/// A test run can allocate gigabytes in seconds; this bounds how far past
/// the budget it gets before it's noticed.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3);
const TERM_GRACE: Duration = Duration::from_secs(5);
/// An unanswered request holds the agent's turn open, so it can't wait forever.
const REQUEST_WAIT: Duration = Duration::from_secs(30 * 60);
const MAX_GRANT_MINUTES: u64 = 8 * 60;

fn budget_from_env() -> Option<u64> {
    match std::env::var("TETHYS_MEM_BUDGET") {
        Err(_) => Some(DEFAULT_BUDGET_MB),
        Ok(v) if v.eq_ignore_ascii_case("off") => None,
        Ok(v) => match v.parse::<u64>() {
            Ok(gb) if gb > 0 => Some(gb * 1024),
            _ => {
                warn!(value = %v, "TETHYS_MEM_BUDGET not `off` or a positive number of GB; using default");
                Some(DEFAULT_BUDGET_MB)
            }
        },
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OverrideRequest {
    pub workspace_id: WorkspaceId,
    pub session_id: SessionId,
    pub gb: u64,
    pub minutes: u64,
    pub reason: String,
}

struct Pending {
    request: OverrideRequest,
    decide: oneshot::Sender<bool>,
}

struct Grant {
    budget_mb: u64,
    until: Instant,
}

/// The default budget, the raises the user approved, and the requests
/// waiting on them. A workspace runs one session, so it has at most one
/// request waiting.
pub struct Budgets {
    default_mb: Option<u64>,
    grants: Mutex<HashMap<SessionId, Grant>>,
    pending: Mutex<HashMap<WorkspaceId, Pending>>,
    app: AppHandle,
}

pub enum Decision {
    Approved { until: chrono::DateTime<chrono::Local> },
    Denied,
}

impl Budgets {
    pub fn new(app: AppHandle) -> Self {
        let default_mb = budget_from_env();
        if default_mb.is_none() {
            info!("memory budget disabled by TETHYS_MEM_BUDGET=off");
        }
        Self {
            default_mb,
            grants: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            app,
        }
    }

    pub fn default_mb(&self) -> Option<u64> {
        self.default_mb
    }

    fn budget_for(&self, session_id: &str, default_mb: u64) -> u64 {
        let mut grants = self.grants.lock().unwrap();
        let now = Instant::now();
        grants.retain(|_, g| g.until > now);
        grants.get(session_id).map_or(default_mb, |g| g.budget_mb.max(default_mb))
    }

    /// Waits for the user. The sidebar marks the workspace until they answer.
    pub async fn request(&self, request: OverrideRequest) -> AppResult<Decision> {
        let Some(default_mb) = self.default_mb else {
            return Err(AppError::Other("the memory budget is switched off".into()));
        };
        if request.gb * 1024 <= default_mb {
            return Err(AppError::Other(format!(
                "the budget is already {} GB; ask for more than that",
                default_mb / 1024
            )));
        }
        if request.minutes == 0 || request.minutes > MAX_GRANT_MINUTES {
            return Err(AppError::Other(format!(
                "minutes must be between 1 and {MAX_GRANT_MINUTES}"
            )));
        }

        let (decide, decided) = oneshot::channel();
        let workspace_id = request.workspace_id.clone();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.contains_key(&workspace_id) {
                return Err(AppError::Other(
                    "a request from this workspace is already waiting on the user".into(),
                ));
            }
            info!(
                %workspace_id,
                gb = request.gb,
                minutes = request.minutes,
                reason = %request.reason,
                "memory budget raise requested"
            );
            pending.insert(workspace_id.clone(), Pending { request: request.clone(), decide });
        }
        self.publish();

        let answer = tokio::time::timeout(REQUEST_WAIT, decided).await;
        if self.pending.lock().unwrap().remove(&workspace_id).is_some() {
            self.publish();
        }
        match answer {
            Ok(Ok(true)) => {
                let length = Duration::from_secs(request.minutes * 60);
                self.grants.lock().unwrap().insert(
                    request.session_id,
                    Grant { budget_mb: request.gb * 1024, until: Instant::now() + length },
                );
                let until = chrono::Local::now()
                    + chrono::TimeDelta::from_std(length).unwrap_or_default();
                Ok(Decision::Approved { until })
            }
            Ok(Ok(false)) => Ok(Decision::Denied),
            Ok(Err(_)) => Err(AppError::Other("the request was dropped".into())),
            Err(_) => Err(AppError::Other(format!(
                "the user didn't answer within {} minutes",
                REQUEST_WAIT.as_secs() / 60
            ))),
        }
    }

    pub fn decide(&self, workspace_id: &str, approve: bool) -> AppResult<()> {
        let pending = self
            .pending
            .lock()
            .unwrap()
            .remove(workspace_id)
            .ok_or_else(|| AppError::Other("no memory budget request is waiting".into()))?;
        info!(workspace_id, approve, "memory budget request decided");
        let _ = pending.decide.send(approve);
        self.publish();
        Ok(())
    }

    pub fn pending(&self) -> Vec<OverrideRequest> {
        self.pending
            .lock()
            .unwrap()
            .values()
            .map(|p| p.request.clone())
            .collect()
    }

    fn publish(&self) {
        let _ = self.app.emit("session:budget_requests_changed", ());
    }
}

/// A dedicated thread: a sweep is blocking syscalls and a grace-period sleep.
pub fn spawn(supervisor: Arc<SessionSupervisor>, budgets: Arc<Budgets>) {
    let Some(default_mb) = budgets.default_mb() else { return };
    std::thread::spawn(move || {
        let own = Tags::own();
        // Kept across sweeps so each process's environment is read once.
        let mut sys = System::new();
        loop {
            sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing()
                    .with_cmd(UpdateKind::OnlyIfNotSet)
                    .with_environ(UpdateKind::OnlyIfNotSet),
            );
            let procs = processes(&sys, &own);
            for kill in over_budget(&procs, |s| budgets.budget_for(s, default_mb)) {
                enforce(&supervisor, &kill);
            }
            std::thread::sleep(SWEEP_INTERVAL);
        }
    });
}

struct Proc {
    pid: Pid,
    parent: Option<Pid>,
    start_time: u64,
    /// `None` for anything untagged or spared.
    session: Option<String>,
    is_tmux: bool,
    footprint_mb: u64,
    command: String,
}

fn processes(sys: &System, own: &Tags) -> Vec<Proc> {
    sys.processes()
        .values()
        .map(|p| {
            let tags = Tags::of_env(p.environ().iter().cloned());
            let command = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            // `pnpm tauri dev` run from inside a session puts Tethys's own
            // tree under that session's tag.
            let spared = tags.shares_any(own) || command.contains("memwatch.sh");
            let session = tags.session_id.filter(|_| !spared);
            Proc {
                pid: p.pid(),
                parent: p.parent(),
                start_time: p.start_time(),
                footprint_mb: if session.is_some() { footprint_mb(p.pid()) } else { 0 },
                session,
                is_tmux: p.name() == "tmux",
                command: command.chars().take(200).collect(),
            }
        })
        .collect()
}

/// What Activity Monitor shows. RSS leaves out compressed pages, so it
/// understates most exactly when memory is short.
fn footprint_mb(pid: Pid) -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: RUSAGE_INFO_V2 writes exactly one `rusage_info_v2`.
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid.as_u32() as libc::c_int,
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast(),
        )
    };
    if rc != 0 {
        return 0;
    }
    // SAFETY: initialised by the successful call above.
    unsafe { info.assume_init() }.ri_phys_footprint / (1024 * 1024)
}

#[derive(Default)]
struct Job<'a> {
    members: Vec<&'a Proc>,
    mb: u64,
}

struct Kill<'a> {
    session: &'a str,
    session_mb: u64,
    budget_mb: u64,
    job: Job<'a>,
}

/// The biggest job of each session over budget. One per sweep: the next
/// sweep sees what killing it freed.
fn over_budget(procs: &[Proc], budget_mb: impl Fn(&str) -> u64) -> Vec<Kill<'_>> {
    let by_pid: HashMap<Pid, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let parent_of = |p: &Proc| p.parent.and_then(|pp| by_pid.get(&pp).copied());
    // tmux runs the agent as the pane's process.
    let is_agent = |p: &Proc| parent_of(p).is_some_and(|pp| pp.is_tmux);
    let job_root = |p: &Proc, session: &str| {
        let mut cur = p;
        // A racing table could loop; no real chain is this deep.
        for _ in 0..64 {
            match parent_of(cur) {
                Some(pp) if pp.session.as_deref() == Some(session) && !is_agent(pp) => cur = pp,
                _ => break,
            }
        }
        cur.pid
    };

    let mut sessions: HashMap<&str, HashMap<Pid, Job>> = HashMap::new();
    for p in procs {
        let Some(session) = p.session.as_deref() else { continue };
        if is_agent(p) {
            continue;
        }
        let job = sessions
            .entry(session)
            .or_default()
            .entry(job_root(p, session))
            .or_default();
        job.members.push(p);
        job.mb += p.footprint_mb;
    }

    sessions
        .into_iter()
        .filter_map(|(session, jobs)| {
            let session_mb = jobs.values().map(|j| j.mb).sum();
            let budget_mb = budget_mb(session);
            if session_mb <= budget_mb {
                return None;
            }
            let job = jobs.into_values().max_by_key(|j| j.mb)?;
            Some(Kill { session, session_mb, budget_mb, job })
        })
        .collect()
}

fn enforce(supervisor: &SessionSupervisor, kill: &Kill<'_>) {
    let Some(biggest) = kill.job.members.iter().max_by_key(|p| p.footprint_mb) else {
        return;
    };
    info!(
        session_id = kill.session,
        session_mb = kill.session_mb,
        job_mb = kill.job.mb,
        budget_mb = kill.budget_mb,
        processes = kill.job.members.len(),
        command = %biggest.command,
        "session over memory budget; killing its biggest job"
    );
    // Before the signal: the killed command ends a tool call, and that
    // call's hook is what carries this to the agent.
    supervisor.record_budget_kill(
        kill.session,
        notice(&biggest.command, kill.job.mb, kill.session_mb, kill.budget_mb),
    );

    let targets: Vec<(Pid, u64)> = kill.job.members.iter().map(|p| (p.pid, p.start_time)).collect();
    strays::signal_if_same(&targets, Signal::Term);
    std::thread::sleep(TERM_GRACE);
    let survivors = strays::signal_if_same(&targets, Signal::Kill);
    if survivors > 0 {
        warn!(count = survivors, "over-budget processes ignored SIGTERM; sent SIGKILL");
    }
}

fn notice(command: &str, job_mb: u64, session_mb: u64, budget_mb: u64) -> String {
    let gb = |mb: u64| mb as f64 / 1024.0;
    format!(
        "Tethys killed `{command}` to protect the machine's memory. The processes this \
         session started held {:.1} GB together, over its {:.1} GB budget, and this job \
         held {:.1} GB of that. The command did not fail on its own, and rerunning it \
         unchanged will be killed again. A narrower run (fewer test files, fewer \
         workers) may fit. If it genuinely needs the memory, the tethys \
         request_memory_budget tool asks the user for a higher budget for a while.",
        gb(session_mb),
        gb(budget_mb),
        gb(job_mb),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TMUX: u32 = 2;

    fn proc(pid: u32, parent: u32, session: Option<&str>, mb: u64) -> Proc {
        Proc {
            pid: Pid::from_u32(pid),
            parent: Some(Pid::from_u32(parent)),
            start_time: 0,
            session: session.map(String::from),
            is_tmux: false,
            footprint_mb: mb,
            command: String::new(),
        }
    }

    fn tmux() -> Proc {
        Proc { is_tmux: true, ..proc(TMUX, 1, None, 0) }
    }

    fn killed(procs: &[Proc], budget_mb: u64) -> Vec<(String, Vec<u32>)> {
        let mut out: Vec<_> = over_budget(procs, |_| budget_mb)
            .into_iter()
            .map(|k| {
                let mut pids: Vec<u32> = k.job.members.iter().map(|p| p.pid.as_u32()).collect();
                pids.sort();
                (k.session.to_string(), pids)
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn the_agent_counts_for_nothing_and_is_never_killed() {
        let procs = [tmux(), proc(10, TMUX, Some("s"), 50_000), proc(11, 10, Some("s"), 100)];
        assert!(killed(&procs, 1_000).is_empty());
    }

    #[test]
    fn the_biggest_job_goes_whole() {
        let procs = [
            tmux(),
            proc(10, TMUX, Some("s"), 300),
            // A Bash tool call: shell, then the test runner and its workers.
            proc(20, 10, Some("s"), 5),
            proc(21, 20, Some("s"), 600),
            proc(22, 21, Some("s"), 600),
            // An MCP server, smaller.
            proc(30, 10, Some("s"), 200),
        ];
        assert_eq!(killed(&procs, 1_000), [("s".to_string(), vec![20, 21, 22])]);
    }

    #[test]
    fn under_budget_is_left_alone() {
        let procs = [tmux(), proc(10, TMUX, Some("s"), 0), proc(20, 10, Some("s"), 999)];
        assert!(killed(&procs, 1_000).is_empty());
    }

    /// `nohup … &` reparents to launchd but keeps the tag.
    #[test]
    fn a_detached_job_still_counts() {
        let procs = [
            tmux(),
            proc(10, TMUX, Some("s"), 0),
            proc(20, 1, Some("s"), 700),
            proc(21, 20, Some("s"), 700),
            proc(30, 10, Some("s"), 100),
        ];
        assert_eq!(killed(&procs, 1_000), [("s".to_string(), vec![20, 21])]);
    }

    #[test]
    fn sessions_are_budgeted_separately() {
        let procs = [
            tmux(),
            proc(10, TMUX, Some("a"), 0),
            proc(11, 10, Some("a"), 800),
            proc(20, TMUX, Some("b"), 0),
            proc(21, 20, Some("b"), 800),
            proc(30, TMUX, Some("c"), 0),
            proc(31, 30, Some("c"), 1_200),
        ];
        assert_eq!(killed(&procs, 1_000), [("c".to_string(), vec![31])]);
    }

    #[test]
    fn untagged_processes_are_never_counted() {
        let procs = [tmux(), proc(10, TMUX, Some("s"), 0), proc(20, 10, None, 50_000)];
        assert!(killed(&procs, 1_000).is_empty());
    }

    #[test]
    fn a_parent_cycle_does_not_hang_the_walk() {
        let procs = [proc(40, 41, Some("s"), 600), proc(41, 40, Some("s"), 600)];
        assert_eq!(killed(&procs, 1_000).len(), 1);
    }

    #[test]
    fn each_session_is_held_to_its_own_budget() {
        let procs = [
            tmux(),
            proc(10, TMUX, Some("raised"), 0),
            proc(11, 10, Some("raised"), 1_500),
            proc(20, TMUX, Some("default"), 0),
            proc(21, 20, Some("default"), 1_500),
        ];
        let kills = over_budget(&procs, |s| if s == "raised" { 2_000 } else { 1_000 });
        let sessions: Vec<&str> = kills.iter().map(|k| k.session).collect();
        assert_eq!(sessions, ["default"]);
    }
}
