mod agent;
mod artifacts;
mod branch_name;
mod budget;
mod child_events;
mod child_env;
mod agent_bin;
mod agent_cmd;
mod claude_local;
mod codex_trust;
mod commands;
mod error;
mod git;
mod github;
mod handoff;
mod hook_install;
mod hook_listener;
mod inprogress;
mod job;
mod logging;
mod mcp;
mod memwatch;
mod paths;
mod probe;
mod provision;
mod provision_queue;
mod pty;
mod purge;
mod reconcile;
mod registry;
mod repo_env;
mod scrollback;
mod session_inbox;
mod sessions;
mod shipping;
mod setup;
mod shell;
mod state;
mod store;
mod strays;
mod theme;
mod tmux;
#[cfg(test)]
mod test_support;
mod turn;
mod ui_socket;
mod workspace_doc;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

use crate::agent_bin::AgentBins;
use crate::artifacts::ArtifactStore;
use crate::budget::Budgets;
use crate::github::poller::LiveEvents;
use crate::github::GithubPoller;
use crate::inprogress::InProgressWorkspaces;
use crate::mcp::McpLaunch;
use crate::paths::Paths;
use crate::provision_queue::ProvisionQueue;
use crate::purge::Purger;
use crate::registry::RegistryLoad;
use crate::sessions::SessionSupervisor;
use crate::shipping::Shipper;
use crate::store::Store;
use crate::ui_socket::Events;

/// Everything a UI command can reach.
pub struct Daemon {
    pub store: Arc<Store>,
    pub registry: Arc<RegistryLoad>,
    pub paths: Paths,
    pub events: Events,
    pub supervisor: Arc<SessionSupervisor>,
    pub artifacts: Arc<ArtifactStore>,
    pub budgets: Arc<Budgets>,
    pub poller: Arc<GithubPoller>,
    pub purger: Arc<Purger>,
    pub shipper: Arc<Shipper>,
    pub in_progress: InProgressWorkspaces,
    pub queue: ProvisionQueue,
    pub agent_bins: AgentBins,
    /// Empty when tmux didn't resolve at boot.
    pub tmux_bin: PathBuf,
    pub mcp: Option<McpLaunch>,
}

/// Runs until the shell that spawned it goes away; returns the exit code.
pub async fn run() -> i32 {
    let pipes = match ShellPipes::take() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tethysd: could not set up stdio: {e}");
            return 1;
        }
    };
    let paths = match Paths::resolve() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tethysd: could not resolve the data dir: {e}");
            return 1;
        }
    };
    // Before anything writes there: binding over a live Tethys's sockets
    // would take its sessions' hooks.
    if hook_listener::is_served(&paths.hook_socket()) {
        eprintln!(
            "tethysd: another Tethys is serving {}; not starting",
            paths.data_dir.display()
        );
        return 1;
    }

    if let Err(e) = std::fs::create_dir_all(paths.logs_dir()) {
        eprintln!("tethysd: could not create the logs dir: {e}");
        return 1;
    }
    let _log_guard = logging::init(&paths.logs_dir());
    info!(data_dir = ?paths.data_dir, isolated = paths.isolated, "tethys starting up");

    let daemon = match boot(paths).await {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "boot failed");
            return 1;
        }
    };

    let ui_socket = daemon.paths.ui_socket();
    let listener = match ui_socket::bind(&ui_socket) {
        Ok(l) => l,
        Err(e) => {
            error!(error = %e, path = %ui_socket.display(), "ui socket failed to bind");
            return 1;
        }
    };
    tokio::spawn(ui_socket::serve(listener, daemon.clone()));
    if let Err(e) = pipes.announce_ready() {
        error!(error = %e, "could not tell the shell we're ready");
        return 1;
    }

    shutdown_requested(pipes.control).await;
    if let Err(e) = daemon.store.flush().await {
        error!(error = %e, "final state flush failed");
    }
    let _ = std::fs::remove_file(&ui_socket);
    info!("tethys shut down");
    0
}

async fn boot(paths: Paths) -> Result<Arc<Daemon>, Box<dyn std::error::Error>> {
    if paths.isolated {
        info!("isolated: leaving the real tmux server, Claude settings, codex config and memwatch alone");
    } else {
        // Before anything heavy, so a hang during boot is still sampled.
        memwatch::spawn();
    }
    if let Some(label) = paths.isolated_tmux_label() {
        info!(label, "isolated tmux server");
        tmux::set_socket_label(label);
    }

    if let Err(e) = registry::write_schema(&paths.repos_schema_file()) {
        error!(error = %e, "failed to write repos.schema.json");
    }

    let registry_load = RegistryLoad::load(&paths.repos_config_file());
    match &registry_load {
        RegistryLoad::Ok { .. } => info!("registry ok"),
        RegistryLoad::Missing { path } => {
            info!(?path, "repos.toml missing — user will be prompted to create it")
        }
        RegistryLoad::Invalid { path, error } => {
            error!(?path, %error, "repos.toml failed to load")
        }
    }
    let registry = Arc::new(registry_load);

    let events = Events::default();
    let store = Store::load(paths.state_file(), Box::new(events.clone()))
        .await
        .inspect_err(|e| error!(error = %e, "failed to load store"))?;

    let agent_bins = AgentBins::resolve_all();

    let tmux_bin = match tmux::resolve() {
        Ok(path) => {
            tmux::ensure_server_init(&path);
            reap_orphan_tmux_sessions(&path, &store).await;
            path
        }
        Err(e) => {
            warn!(error = %e, "tmux binary not resolved at startup");
            PathBuf::new()
        }
    };

    if !paths.isolated {
        install_claude_hooks(&paths);
    }

    let artifacts = Arc::new(ArtifactStore::new(events.clone(), store.clone()));
    let supervisor = Arc::new(SessionSupervisor::new(
        events.clone(),
        store.clone(),
        artifacts.clone(),
    ));

    let budgets = Arc::new(Budgets::new(events.clone()));
    if !tmux_bin.as_os_str().is_empty() {
        prewarm_live_sessions(&supervisor, &tmux_bin, &store).await;
        // Both sweeps act on every tagged process on the machine. An isolated
        // run can't tell the real sessions' processes from its own, and the
        // stray sweep, asking only its own tmux server, would read them all
        // as dead.
        if !paths.isolated {
            strays::spawn(tmux_bin.clone());
        }
    }
    if !paths.isolated {
        budget::spawn(supervisor.clone(), budgets.clone());
    }

    hook_listener::start(&paths.hook_socket(), supervisor.clone())
        .await
        .inspect_err(|e| error!(error = %e, "hook listener failed to start"))?;

    probe::spawn(supervisor.clone());

    let in_progress = InProgressWorkspaces::new();
    let queue = ProvisionQueue::new();
    let mcp_launch = McpLaunch::resolve(&paths, &registry);

    let shipper = Arc::new(Shipper {
        store: store.clone(),
        supervisor: supervisor.clone(),
        tmux_bin: tmux_bin.clone(),
        agent_bins: agent_bins.clone(),
        paths: paths.clone(),
        mcp: mcp_launch.clone(),
    });

    let handoff = Arc::new(handoff::Handoff {
        store: store.clone(),
        registry: registry.clone(),
        paths: paths.clone(),
        in_progress: in_progress.clone(),
        queue: queue.clone(),
        supervisor: supervisor.clone(),
        tmux_bin: tmux_bin.clone(),
        agent_bins: agent_bins.clone(),
        mcp: mcp_launch.clone(),
    });
    let mcp_services = mcp::McpServices {
        handoff,
        store: store.clone(),
        registry: registry.clone(),
        paths: paths.clone(),
        queue: queue.clone(),
        budgets: budgets.clone(),
    };
    if let Err(e) = mcp::listen(&paths.mcp_socket(), mcp_services).await {
        error!(error = %e, "mcp listener failed to start");
    }

    let poller = Arc::new(GithubPoller::new(
        store.clone(),
        registry.clone(),
        LiveEvents {
            events: events.clone(),
            shipper: shipper.clone(),
        },
    ));
    let poller_for_probe = poller.clone();
    tokio::spawn(async move {
        match poller_for_probe.probe_login().await {
            Some(login) => info!(login, "gh authenticated"),
            None => info!("gh auth probe failed — polling will retry"),
        }
    });
    tokio::spawn(poller.clone().run());

    let purger = Arc::new(Purger::new(
        store.clone(),
        paths.clone(),
        registry.clone(),
        events.clone(),
    ));
    tokio::spawn(purger.clone().run());

    let store_for_docs = store.clone();
    let paths_for_docs = paths.clone();
    let registry_for_docs = registry.clone();
    tokio::spawn(async move {
        refresh_workspace_docs(&store_for_docs, &registry_for_docs, &paths_for_docs).await;
    });

    Ok(Arc::new(Daemon {
        store,
        registry,
        paths,
        events,
        supervisor,
        artifacts,
        budgets,
        poller,
        purger,
        shipper,
        in_progress,
        queue,
        agent_bins,
        tmux_bin,
        mcp: mcp_launch,
    }))
}

fn install_claude_hooks(paths: &Paths) {
    let Some(hook_bin) = hook_install::bundled_hook_bin_or_warn() else {
        return;
    };
    let Some(settings) = paths::claude_settings_path() else {
        warn!("HOME not set; skipping hook install");
        return;
    };
    if let Err(e) = hook_install::install(&settings, &paths.claude_settings_lock(), &hook_bin) {
        warn!(error = %e, "hook install failed");
    }
}

/// The shell's pipes, kept from every child `Command` spawns: they inherit
/// fds 0 and 1, and one writing to stdout could land ahead of `ready`.
struct ShellPipes {
    /// Never written to; EOF means the shell is gone.
    control: File,
    announce: File,
}

impl ShellPipes {
    fn take() -> std::io::Result<Self> {
        let check = |rc: libc::c_int| {
            if rc < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(rc)
            }
        };
        // SAFETY: plain fd juggling on fds this process owns; each new fd is
        // handed to exactly one `File`.
        unsafe {
            let control = check(libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3))?;
            let announce = check(libc::fcntl(1, libc::F_DUPFD_CLOEXEC, 3))?;
            let null = check(libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY))?;
            check(libc::dup2(null, 0))?;
            libc::close(null);
            check(libc::dup2(2, 1))?;
            Ok(Self {
                control: File::from_raw_fd(control),
                announce: File::from_raw_fd(announce),
            })
        }
    }

    fn announce_ready(&self) -> std::io::Result<()> {
        let mut out = &self.announce;
        out.write_all(b"ready\n")?;
        out.flush()
    }
}

async fn shutdown_requested(mut control: File) {
    let (closed_tx, closed) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            match control.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = closed_tx.send(());
    });
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        let _ = closed.await;
        info!("stdin closed; shutting down");
        return;
    };
    tokio::select! {
        _ = closed => info!("stdin closed; shutting down"),
        _ = term.recv() => info!("SIGTERM; shutting down"),
        _ = int.recv() => info!("SIGINT; shutting down"),
    }
}

/// How edits to `repos.toml` reach workspaces already on disk.
async fn refresh_workspace_docs(store: &Arc<Store>, registry: &RegistryLoad, paths: &Paths) {
    let Ok(reg) = registry.require() else { return };
    let workspaces = store.read(|s| s.workspaces.clone()).await;
    let mut written = 0usize;
    for ws in workspaces.iter().filter(|w| w.deleted_at.is_none()) {
        match workspace_doc::regenerate(ws, reg, paths).await {
            Ok(Some(_)) => written += 1,
            Ok(None) => {}
            Err(e) => {
                warn!(workspace = %ws.id, error = %e, "failed to refresh workspace CLAUDE.md")
            }
        }
    }
    if written > 0 {
        info!(count = written, "refreshed workspace CLAUDE.md files");
    }
}

/// So switching into a workspace shows its live terminal without a
/// "Dormant / Resume" flash.
async fn prewarm_live_sessions(
    supervisor: &Arc<SessionSupervisor>,
    tmux_bin: &std::path::Path,
    store: &Arc<Store>,
) {
    struct PrewarmCandidate {
        session_id: String,
        workspace_id: String,
        cwd: std::path::PathBuf,
        runtime_state: Option<crate::state::SessionRuntimeState>,
        notification_type: Option<String>,
        turn_acknowledged: bool,
    }

    let candidates: Vec<PrewarmCandidate> = store
        .read(|s| {
            s.workspaces
                .iter()
                .filter_map(|ws| {
                    let meta = ws.session.as_ref()?;
                    Some(PrewarmCandidate {
                        session_id: meta.id.clone(),
                        workspace_id: ws.id.clone(),
                        cwd: meta.cwd.clone(),
                        runtime_state: meta.runtime_state,
                        notification_type: meta.notification_type.clone(),
                        turn_acknowledged: meta.turn_acknowledged,
                    })
                })
                .collect()
        })
        .await;

    for c in candidates {
        if !tmux::has_session(tmux_bin, &c.session_id) {
            continue;
        }
        match supervisor.reattach_tmux(c.session_id.clone(), c.workspace_id, &c.cwd, tmux_bin) {
            Ok(_) => {
                info!(session_id = %c.session_id, "pre-warmed live tmux session");
                if let Some(state) = c.runtime_state {
                    supervisor.restore_turn(
                        &c.session_id,
                        state,
                        c.notification_type,
                        c.turn_acknowledged,
                    );
                }
            }
            Err(e) => warn!(session_id = %c.session_id, error = %e, "pre-warm reattach failed"),
        }
    }
}

async fn reap_orphan_tmux_sessions(tmux_bin: &std::path::Path, store: &Arc<Store>) {
    let known: std::collections::HashSet<String> = store
        .read(|s| {
            s.workspaces
                .iter()
                .filter_map(|w| w.session.as_ref().map(|sess| sess.id.clone()))
                .collect::<std::collections::HashSet<_>>()
        })
        .await;

    for name in tmux::list_sessions(tmux_bin) {
        if known.contains(&name) {
            continue;
        }
        warn!(session = %name, "killing orphaned tmux session");
        tmux::kill_session(tmux_bin, &name);
    }
}
