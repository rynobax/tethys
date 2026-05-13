//! Main-stack auto-start. The worktree's django needs the main stack's
//! postgres/redis on the standard ports — `network_mode: host` means
//! "localhost" inside the container is the host's, so worktree django
//! connects directly to `localhost:5432` etc. If the main stack isn't
//! running, the worktree's django will fail-loop until we bring it up.

use std::process::Command;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::config::OrchestratorConfig;
use super::ports;

pub struct MainStackStatus {
    pub was_started: bool,
    pub waited: Duration,
}

/// If postgres+redis are already bound, fast-path return. Otherwise run
/// `docker compose up -d` in the main-stack dir, retry once after
/// `docker rm -f` of the hardcoded-named orphan containers on conflict,
/// then poll for the health ports up to 60s.
pub fn ensure_running(cfg: &OrchestratorConfig) -> Result<MainStackStatus, String> {
    if all_health_ports_bound(cfg) {
        return Ok(MainStackStatus {
            was_started: false,
            waited: Duration::ZERO,
        });
    }

    if !cfg.main_stack_dir.exists() {
        return Err(format!(
            "main stack dir not found: {}",
            cfg.main_stack_dir.display()
        ));
    }

    info!(dir = %cfg.main_stack_dir.display(), "starting main stack");
    let mut output = compose_up(cfg);
    if !output.status.success() {
        // Most common cause is a leftover container holding a hardcoded
        // name. Force-remove the suspects and retry once.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already in use") || stderr.contains("Conflict") {
            warn!("compose up hit a name conflict — removing orphans and retrying");
            for name in &cfg.main_stack_orphan_container_names {
                let _ = Command::new("docker").args(["rm", "-f", name]).output();
            }
            output = compose_up(cfg);
        }
    }
    if !output.status.success() {
        return Err(format!(
            "docker compose up -d failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let started = Instant::now();
    let limit = Duration::from_secs(60);
    while started.elapsed() < limit {
        if all_health_ports_bound(cfg) {
            return Ok(MainStackStatus {
                was_started: true,
                waited: started.elapsed(),
            });
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "main stack didn't bind ports within {}s",
        limit.as_secs()
    ))
}

fn all_health_ports_bound(cfg: &OrchestratorConfig) -> bool {
    cfg.main_stack_health_ports
        .iter()
        .all(|&p| ports::is_port_bound(p))
}

fn compose_up(cfg: &OrchestratorConfig) -> std::process::Output {
    Command::new("docker")
        .args(["compose", "up", "-d"])
        .current_dir(&cfg.main_stack_dir)
        .output()
        // If docker itself can't be invoked, fabricate a failed Output so
        // the caller's error path runs uniformly.
        .unwrap_or_else(|e| std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: Vec::new(),
            stderr: format!("docker invoke failed: {e}").into_bytes(),
        })
}

/// Restart a specific container by name. Used after the main stack was
/// just (re)started — any worktree django that was running has stale
/// connections to the old postgres process. Best-effort; silent on failure.
pub fn restart_container(name: &str) {
    let _ = Command::new("docker").args(["restart", name]).output();
}
