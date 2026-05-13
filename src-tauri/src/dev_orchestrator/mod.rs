//! Per-worktree dev-server orchestration.
//!
//! Public surface for the rest of Tethys:
//!
//! - [`OrchestratorConfig`] — project-specific knobs. All newlantern-isms
//!   live here behind `OrchestratorConfig::newlantern()`. Swap that for
//!   a config-file loader to support other projects without touching the
//!   rest of the module.
//! - [`prep`] — figure out ports, generate the docker-compose override,
//!   symlink env files, ensure the main stack is up. Returns a
//!   [`PrepResult`] the caller uses to actually spawn FE/BE processes.
//! - [`stop`] — tear down a worktree's dev stack.
//! - [`pressure::current`] — memory-pressure snapshot for the poller.
//! - [`stats::workspace_memory`] — per-worktree RAM (FE rspack + BE
//!   container) for the poller.
//! - [`scrub::scrub_orphan_overrides`] — cleanup of stale override
//!   files from removed worktrees.

pub mod config;
pub mod down;
pub mod env_files;
pub mod main_stack;
pub mod override_file;
pub mod ports;
pub mod pressure;
pub mod scrub;
pub mod stats;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use tracing::{info, warn};

pub use config::OrchestratorConfig;
pub use pressure::{Pressure, SystemMemory};

use crate::state::Workspace;

/// What the orchestrator decided + the inputs the caller needs to
/// actually spawn the FE/BE tmux sessions.
#[derive(Debug, Clone, Serialize)]
pub struct PrepResult {
    pub short: String,
    pub label: String,
    pub fe: ServicePrep,
    /// `None` when we auto-decided to skip starting a worktree BE
    /// (branch has no backend changes, and the caller didn't override).
    pub be: Option<ServicePrep>,
    /// What `NL_PROXY_TARGET` should be set to when spawning FE. Either
    /// the worktree BE URL (if `be.is_some()`) or the main-stack BE URL.
    pub fe_proxy_target: String,
    /// `true` if the branch has any changes under the BE repo's path
    /// relative to master. The caller may surface this to drive a
    /// "Start BE anyway" hint in the UI.
    pub be_changes_present: bool,
    /// Whether the main stack was just started by this prep call (vs.
    /// already running). When `true`, the caller should restart any
    /// existing worktree django containers to refresh their (now
    /// stale) postgres connections.
    pub main_stack_was_started: bool,
    /// Non-fatal warnings to surface to the user (e.g. "main stack
    /// failed to start; FE proxying to master may not work").
    pub warnings: Vec<String>,
}

/// One service the caller will spawn in a tmux session.
#[derive(Debug, Clone, Serialize)]
pub struct ServicePrep {
    /// Deterministic id; doubles as tmux session name.
    pub session_id: String,
    pub cwd: PathBuf,
    pub port: u16,
    /// Shell command to run under a login zsh. The caller should spawn
    /// `tmux new-session -- /bin/zsh -ilc <command>` (or equivalent).
    pub shell_command: String,
    /// Extra env vars to inject into the spawned process.
    pub env: Vec<(String, String)>,
}

/// Caller-facing override for the auto-decide rule.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BeMode {
    /// `git diff master -- backend/` decides.
    #[default]
    Auto,
    /// Always start the worktree BE (used by "Start BE anyway" button).
    ForceInclude,
    /// Never start the worktree BE (used by FE-only context menu).
    ForceExclude,
}

#[derive(Debug, Default)]
pub struct StopReport {
    pub teardown: down::TeardownReport,
    pub short: String,
}

/// Compute the per-worktree short suffix. Prefers the NL ticket number
/// in the branch (e.g. `nathan/nl-6457-foo` → `6457`). Falls back to
/// the first 6 hex chars of a non-crypto hash of the branch.
pub fn short_from_branch(branch: &str) -> String {
    let lower = branch.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if &bytes[i..i + 3] == b"nl-" {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 3 {
                return String::from_utf8_lossy(&bytes[i + 3..j]).into_owned();
            }
        }
        i += 1;
    }
    let mut h = DefaultHasher::new();
    branch.hash(&mut h);
    format!("{:06x}", h.finish() & 0xFFFFFF)
}

/// Human label for the worktree (window/tab titles). `NL-6457` when
/// possible, else first 24 chars of the branch.
pub fn label_from_branch(branch: &str) -> String {
    let short = short_from_branch(branch);
    if short.chars().all(|c| c.is_ascii_digit()) {
        format!("NL-{short}")
    } else {
        // Strip user prefix if any ("nathan/foo" -> "foo") so the label
        // is content-bearing.
        let after_slash = branch.rsplit('/').next().unwrap_or(branch);
        after_slash.chars().take(24).collect()
    }
}

/// Look up the worktree path for a given repo_key. Returns Err if the
/// workspace doesn't have that repo linked — caller should surface
/// "configure this repo first" to the user.
fn worktree_path(ws: &Workspace, repo_key: &str) -> Result<PathBuf, String> {
    ws.repo_links
        .iter()
        .find(|l| l.repo_key == repo_key)
        .map(|l| l.worktree_path.clone())
        .ok_or_else(|| format!("workspace has no '{}' repo linked", repo_key))
}

/// `true` if there are any changes in the BE worktree relative to
/// master. Wraps `git diff --quiet master -- .` — exit zero means clean.
fn has_be_changes(be_dir: &Path, master_branch: &str) -> bool {
    let out = Command::new("git")
        .args(["diff", "--quiet", master_branch, "--", "."])
        .current_dir(be_dir)
        .status();
    match out {
        Ok(s) => !s.success(), // exit nonzero = diff present
        Err(_) => false,
    }
}

/// Top-level prep — port allocation, env links, override file, main
/// stack up. Caller is responsible for spawning the FE/BE processes
/// from `PrepResult`.
pub fn prep(
    cfg: &OrchestratorConfig,
    workspace: &Workspace,
    mode: BeMode,
) -> Result<PrepResult, String> {
    let fe_dir = worktree_path(workspace, &cfg.fe_repo_key)?;
    let be_dir = worktree_path(workspace, &cfg.be_repo_key)?;
    let short = short_from_branch(&workspace.branch);
    let label = label_from_branch(&workspace.branch);

    let mut warnings = Vec::new();

    // Cheap upfront housekeeping.
    let removed = scrub::scrub_orphan_overrides(cfg);
    if !removed.is_empty() {
        info!(count = removed.len(), "scrubbed orphan overrides");
    }

    // Main stack first — we need it for the env-symlink target to
    // exist, AND we want the wait-loop fronted before we spawn FE/BE.
    let main_status = match main_stack::ensure_running(cfg) {
        Ok(s) => Some(s),
        Err(e) => {
            warnings.push(format!("main stack: {e}"));
            None
        }
    };
    let main_stack_was_started = main_status
        .as_ref()
        .map(|s| s.was_started)
        .unwrap_or(false);

    let be_changes = has_be_changes(&be_dir, &cfg.master_branch);
    let want_be = match mode {
        BeMode::Auto => be_changes,
        BeMode::ForceInclude => true,
        BeMode::ForceExclude => false,
    };

    let fe_port = ports::find_free_port_from(cfg.fe_port_start)
        .ok_or_else(|| format!("no free FE port in [{}, +200)", cfg.fe_port_start))?;
    let be_port = ports::find_free_port_from(cfg.be_port_start)
        .ok_or_else(|| format!("no free BE port in [{}, +200)", cfg.be_port_start))?;

    let (be, fe_proxy_target) = if want_be {
        if let Err(e) = env_files::link_into_worktree(cfg, &be_dir) {
            warnings.push(format!("env file link: {e}"));
        }
        if let Err(e) = override_file::write(cfg, &be_dir, &short, be_port) {
            return Err(format!("override file: {e}"));
        }
        let be_session_id = format!("tethys-be-{}", workspace.id);
        (
            Some(ServicePrep {
                session_id: be_session_id,
                cwd: be_dir.clone(),
                port: be_port,
                shell_command: cfg.be_command_template.clone(),
                env: Vec::new(),
            }),
            format!("http://localhost:{be_port}"),
        )
    } else {
        (None, cfg.master_be_url.clone())
    };

    let fe_session_id = format!("tethys-fe-{}", workspace.id);
    let fe = ServicePrep {
        session_id: fe_session_id,
        cwd: fe_dir,
        port: fe_port,
        shell_command: cfg.fe_command(fe_port),
        env: vec![(cfg.fe_proxy_env_var.clone(), fe_proxy_target.clone())],
    };

    Ok(PrepResult {
        short,
        label,
        fe,
        be,
        fe_proxy_target,
        be_changes_present: be_changes,
        main_stack_was_started,
        warnings,
    })
}

/// Tear down a worktree's dev stack.
pub fn stop(
    cfg: &OrchestratorConfig,
    workspace: &Workspace,
) -> Result<StopReport, String> {
    let fe_dir = worktree_path(workspace, &cfg.fe_repo_key)?;
    let be_dir = worktree_path(workspace, &cfg.be_repo_key)?;
    let short = short_from_branch(&workspace.branch);
    let teardown = down::stop_worktree(cfg, &short, &fe_dir, &be_dir);
    Ok(StopReport { teardown, short })
}

/// Borrowed by the caller to mark stale connections in a freshly-
/// restarted main stack. Restart the worktree's django container so
/// its postgres connection pool refreshes.
pub fn restart_worktree_django(cfg: &OrchestratorConfig, short: &str) {
    let container = cfg.be_container(short);
    main_stack::restart_container(&container);
    warn!(container = %container, "restarted worktree django after main stack (re)start");
}
