//! Teardown for a worktree's dev stack. Stops the docker compose
//! project and kills the FE rspack pid. Idempotent — safe to call
//! when the stack is already down.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use tracing::warn;

use super::config::OrchestratorConfig;
use super::override_file;

#[derive(Debug, Default)]
pub struct TeardownReport {
    pub compose_down_ran: bool,
    pub killed_pids: Vec<u32>,
    pub override_removed: bool,
    pub warnings: Vec<String>,
}

pub fn stop_worktree(cfg: &OrchestratorConfig, short: &str, fe_dir: &Path, be_dir: &Path) -> TeardownReport {
    let mut report = TeardownReport::default();
    let project = cfg.compose_project(short);

    // 1. docker compose down by project name (works regardless of
    //    whether the override file is still in place).
    let out = Command::new("docker")
        .args(["compose", "-p", &project, "down"])
        .output();
    match out {
        Ok(o) if o.status.success() => report.compose_down_ran = true,
        Ok(o) => report.warnings.push(format!(
            "compose down: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => report.warnings.push(format!("docker invoke: {e}")),
    }

    // 2. Kill the FE rspack pid for this worktree. Scope by frontend
    //    path so we don't kill another worktree's rspack.
    let pattern = format!("rspack serve.*{}", fe_dir.display());
    let pids = pgrep(&pattern);
    for pid in &pids {
        if signal_pid(*pid, "INT") {
            report.killed_pids.push(*pid);
        }
    }
    if !pids.is_empty() {
        // Give them up to 3s to die on SIGINT, then SIGTERM survivors.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if pgrep(&pattern).is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        for pid in pgrep(&pattern) {
            signal_pid(pid, "TERM");
        }
    }

    // 3. Remove the override file (so next start gets a fresh BE_PORT).
    let path = override_file::override_path(be_dir);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(error = %e, "failed to remove override file");
        } else {
            report.override_removed = true;
        }
    }
    report
}

fn pgrep(pattern: &str) -> Vec<u32> {
    let out = Command::new("pgrep").args(["-f", pattern]).output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

fn signal_pid(pid: u32, sig: &str) -> bool {
    Command::new("kill")
        .args([&format!("-{sig}"), &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
