mod child_env;
mod claude;
mod claude_local;
mod claude_settings;
mod commands;
mod error;
mod git;
mod github;
mod hook_install;
mod hook_listener;
mod inprogress;
mod job;
mod logging;
mod paths;
mod pending_permissions;
mod probe;
mod provision;
mod pty;
mod purge;
mod reconcile;
mod registry;
mod scripts;
mod sessions;
mod setup;
mod shell;
mod state;
mod store;
mod theme;
mod tmux;
mod turn;
mod workspace_doc;

use std::sync::Arc;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tracing::{error, info, warn};

use crate::commands::ClaudeBin;
use crate::github::GithubPoller;
use crate::paths::Paths;
use crate::purge::Purger;
use crate::registry::RegistryLoad;
use crate::scripts::ScriptSupervisor;
use crate::sessions::SessionSupervisor;
use crate::store::{Store, WorkspaceNotifier};
use crate::tmux::TmuxBin;

/// Adapter that turns the `Store`'s workspace-changed notifications into the
/// `workspace:changed` Tauri event the frontend listens for.
struct TauriNotifier(AppHandle);

impl WorkspaceNotifier for TauriNotifier {
    fn workspace_changed(&self, workspace_id: &str) {
        let _ = self
            .0
            .emit("workspace:changed", serde_json::json!({ "workspace_id": workspace_id }));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let handle = app.handle().clone();
            let paths = Paths::from_app(&handle)?;

            let guard = logging::init(&paths.logs_dir());
            app.manage(LoggingGuard(guard));

            info!(data_dir = ?paths.data_dir, "tethys starting up");

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
            app.manage::<Arc<RegistryLoad>>(Arc::new(registry_load));

            // --- state store ------------------------------------------------
            let state_path = paths.state_file();
            let tmp_path = paths.state_tmp_file();
            let store: Arc<Store> = tauri::async_runtime::block_on(async {
                Store::load(
                    state_path,
                    tmp_path,
                    Box::new(TauriNotifier(handle.clone())),
                )
                .await
            })
            .map_err(|e| {
                error!(error = %e, "failed to load store");
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            handle.manage::<Arc<Store>>(store.clone());

            // --- claude binary (non-fatal if missing; surface to UI later) --
            match claude::resolve() {
                Ok(path) => {
                    app.manage(ClaudeBin(path));
                }
                Err(e) => {
                    warn!(error = %e, "claude binary not resolved at startup");
                    // Still manage a placeholder so commands can surface
                    // the error at spawn time rather than panicking.
                    app.manage(ClaudeBin(std::path::PathBuf::new()));
                }
            }

            // --- tmux binary (claude sessions run inside a tmux server so
            // they survive app restarts until reboot).
            let tmux_bin_path = match tmux::resolve() {
                Ok(path) => {
                    tmux::ensure_server_init(&path);
                    reap_orphan_tmux_sessions(&path, &store);
                    let p = path.clone();
                    app.manage(TmuxBin(path));
                    Some(p)
                }
                Err(e) => {
                    warn!(error = %e, "tmux binary not resolved at startup");
                    app.manage(TmuxBin(std::path::PathBuf::new()));
                    None
                }
            };

            // --- hook installer (idempotent) --------------------------------
            if let Some(hook_bin) = hook_install::bundled_hook_bin_or_warn() {
                if let Some(settings) = paths::claude_settings_path() {
                    if let Err(e) = hook_install::install(
                        &settings,
                        &paths.claude_settings_lock(),
                        &hook_bin,
                    ) {
                        warn!(error = %e, "hook install failed");
                    }
                } else {
                    warn!("HOME not set; skipping hook install");
                }
            }

            // --- session supervisor + UDS listener --------------------------
            let supervisor: Arc<SessionSupervisor> =
                Arc::new(SessionSupervisor::new(handle.clone(), store.clone()));
            app.manage(supervisor.clone());

            // --- script supervisor -----------------------------------------
            let script_supervisor: Arc<ScriptSupervisor> =
                Arc::new(ScriptSupervisor::new(handle.clone(), store.clone()));
            app.manage(script_supervisor.clone());

            // Pre-warm live sessions + scripts: for every persisted entry
            // whose tmux pane is still alive, spin up a reattach now so
            // the UI can flip straight to the terminal when the user
            // visits the workspace (no "Dormant / Resume" flash).
            if let Some(path) = tmux_bin_path.as_ref() {
                prewarm_live_sessions(&supervisor, path, &store);
                prewarm_live_scripts(&script_supervisor, path, &store);
            }

            let socket_path = paths.hook_socket();
            let sup_for_listener = supervisor.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = hook_listener::start(&socket_path, sup_for_listener).await {
                    error!(error = %e, "hook listener failed to start");
                }
            });

            // Probe-file reconciler: backstop that corrects hook-derived turn
            // state against Claude Code's own `~/.claude/sessions/*.json`.
            probe::spawn(supervisor.clone());

            // --- github poller ---------------------------------------------
            let registry_for_poller: Arc<RegistryLoad> = app.state::<Arc<RegistryLoad>>().inner().clone();
            let poller = Arc::new(GithubPoller::new(
                store.clone(),
                registry_for_poller,
                handle.clone(),
            ));
            app.manage(poller.clone());
            let poller_for_probe = poller.clone();
            tauri::async_runtime::spawn(async move {
                match poller_for_probe.probe_login().await {
                    Some(login) => info!(login, "gh authenticated"),
                    None => info!("gh auth probe failed — polling will retry"),
                }
            });
            tauri::async_runtime::spawn(poller.clone().run());

            // --- soft-delete purger (hourly) -------------------------------
            let registry_for_purger: Arc<RegistryLoad> =
                app.state::<Arc<RegistryLoad>>().inner().clone();
            let purger = Arc::new(Purger::new(
                store.clone(),
                paths.clone(),
                registry_for_purger,
                handle.clone(),
            ));
            app.manage(purger.clone());
            tauri::async_runtime::spawn(purger.clone().run());

            // --- workspace CLAUDE.md refresh -------------------------------
            let registry_for_docs: Arc<RegistryLoad> =
                app.state::<Arc<RegistryLoad>>().inner().clone();
            let store_for_docs = store.clone();
            let paths_for_docs = paths.clone();
            tauri::async_runtime::spawn(async move {
                refresh_workspace_docs(&store_for_docs, &registry_for_docs, &paths_for_docs).await;
            });

            app.manage(paths);
            app.manage(inprogress::InProgressWorkspaces::new());

            // --- menu (append Theme items under the default View submenu) --
            if let Err(e) = install_menu(&handle) {
                warn!(error = %e, "menu install failed");
            }
            handle.on_menu_event(move |app, event| match event.id().as_ref() {
                "theme_load" => handle_theme_load(app),
                "theme_reset" => handle_theme_reset(app),
                _ => {}
            });

            // --- window focus → force-tick the github poller ---------------
            if let Some(window) = app.get_webview_window("main") {
                let poller_for_focus = poller.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::Focused(true) = event {
                        let p = poller_for_focus.clone();
                        tauri::async_runtime::spawn(async move {
                            p.request_tick().await;
                        });
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_workspaces,
            commands::create_workspace,
            commands::add_repo_to_workspace,
            commands::delete_workspace,
            commands::cancel_delete_workspace,
            commands::archive_workspace,
            commands::unarchive_workspace,
            commands::reorder_workspaces,
            commands::run_purge_now,
            commands::list_system_errors,
            commands::dismiss_system_error,
            commands::list_pending_permissions,
            commands::apply_pending_permission,
            commands::dismiss_pending_permission,
            commands::registry_status,
            commands::github_auth_status,
            commands::github_reprobe_auth,
            commands::attach_pr,
            commands::detach_pr,
            commands::open_repos_config,
            commands::open_in_vscode,
            commands::list_discrepancies,
            commands::remove_orphan_dir,
            commands::forget_workspace,
            commands::list_sessions,
            commands::acknowledge_session_turn,
            commands::start_claude_session,
            commands::resume_claude_session,
            commands::switch_claude_binary,
            commands::set_claude_session_hidden,
            commands::set_workspace_notes,
            commands::attach_session,
            commands::detach_session,
            commands::send_input,
            commands::resize_session,
            commands::list_scripts,
            commands::start_script,
            commands::dismiss_script,
            commands::attach_script,
            commands::detach_script,
            commands::send_input_script,
            commands::resize_script,
            commands::get_theme,
            commands::read_clipboard_file_paths,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

struct LoggingGuard(#[allow(dead_code)] tracing_appender::non_blocking::WorkerGuard);

/// Rewrite the generated `CLAUDE.md` at every live workspace's root. Runs once
/// per boot, which is how workspaces created before the file existed get one and
/// how edits to `claude_notes` in `repos.toml` reach workspaces already on disk.
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

/// For every persisted `ClaudeSessionMeta` whose tmux pane is still
/// alive, spawn a reattach client now. This means `list_sessions` will
/// return `running: true` for those sessions by the time the frontend
/// asks, so switching into a workspace shows the terminal immediately
/// rather than flashing the "Dormant / Resume" state.
fn prewarm_live_sessions(
    supervisor: &Arc<SessionSupervisor>,
    tmux_bin: &std::path::Path,
    store: &Arc<Store>,
) {
    struct PrewarmCandidate {
        session_id: String,
        workspace_id: String,
        repo_key: Option<String>,
        cwd: std::path::PathBuf,
        runtime_state: Option<crate::state::SessionRuntimeState>,
        notification_type: Option<String>,
        turn_acknowledged: bool,
    }

    let candidates: Vec<PrewarmCandidate> = tauri::async_runtime::block_on(async {
        store
            .read(|s| {
                let mut out = Vec::new();
                for ws in &s.workspaces {
                    for meta in &ws.sessions {
                        out.push(PrewarmCandidate {
                            session_id: meta.id.clone(),
                            workspace_id: ws.id.clone(),
                            repo_key: meta.repo_key.clone(),
                            cwd: meta.cwd.clone(),
                            runtime_state: meta.runtime_state,
                            notification_type: meta.notification_type.clone(),
                            turn_acknowledged: meta.turn_acknowledged,
                        });
                    }
                }
                out
            })
            .await
    });

    for c in candidates {
        if !tmux::has_session(tmux_bin, &c.session_id) {
            continue;
        }
        match supervisor.reattach_tmux(
            c.session_id.clone(),
            c.workspace_id,
            c.repo_key,
            &c.cwd,
            tmux_bin,
        ) {
            Ok(_) => {
                info!(session_id = %c.session_id, "pre-warmed live tmux session");
                // Restore the last persisted turn state so the dot survives
                // restarts. `reattach_tmux` seeds Working for a pane that may
                // be mid-response; override it when we have a better answer.
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

/// For every persisted `ScriptRunMeta` whose tmux session is still alive,
/// reattach now so the chip shows running on app start. Entries whose tmux
/// session is gone (e.g. machine reboot wiped the server) are dropped from
/// state — there's no surviving process to reconnect to.
fn prewarm_live_scripts(
    supervisor: &Arc<ScriptSupervisor>,
    tmux_bin: &std::path::Path,
    store: &Arc<Store>,
) {
    struct Candidate {
        id: String,
        workspace_id: String,
        repo_key: String,
        script_name: String,
        command: String,
        cwd: std::path::PathBuf,
        started_at: chrono::DateTime<chrono::Utc>,
    }

    let candidates: Vec<Candidate> = tauri::async_runtime::block_on(async {
        store
            .read(|s| {
                let mut out = Vec::new();
                for ws in &s.workspaces {
                    for run in &ws.script_runs {
                        out.push(Candidate {
                            id: run.id.clone(),
                            workspace_id: ws.id.clone(),
                            repo_key: run.repo_key.clone(),
                            script_name: run.script_name.clone(),
                            command: run.command.clone(),
                            cwd: run.cwd.clone(),
                            started_at: run.started_at,
                        });
                    }
                }
                out
            })
            .await
    });

    let mut to_drop: Vec<(String, String)> = Vec::new();
    for c in candidates {
        if !tmux::has_session(tmux_bin, &c.id) {
            to_drop.push((c.workspace_id, c.id));
            continue;
        }
        match supervisor.reattach_tmux(
            c.id.clone(),
            c.workspace_id.clone(),
            c.repo_key,
            c.script_name,
            c.command,
            &c.cwd,
            tmux_bin,
            c.started_at,
        ) {
            Ok(_) => info!(script_id = %c.id, "pre-warmed live script"),
            Err(e) => {
                warn!(script_id = %c.id, error = %e, "script reattach failed");
                to_drop.push((c.workspace_id, c.id));
            }
        }
    }

    if !to_drop.is_empty() {
        let store_for_drop = store.clone();
        tauri::async_runtime::block_on(async move {
            for (ws_id, script_id) in &to_drop {
                let _ = store_for_drop
                    .update_workspace(ws_id, |ws| {
                        ws.script_runs.retain(|m| &m.id != script_id);
                        Ok(())
                    })
                    .await;
            }
        });
    }
}

/// Kill any tmux session on our private server whose name isn't a known
/// `ClaudeSessionMeta.id` or `ScriptRunMeta.id`. Catches leftovers from app
/// crashes between spawn and state.json flush, and from workspaces that
/// were deleted while their tmux sessions were still alive.
fn reap_orphan_tmux_sessions(tmux_bin: &std::path::Path, store: &Arc<Store>) {
    let known: std::collections::HashSet<String> = tauri::async_runtime::block_on(async {
        store
            .read(|s| {
                s.workspaces
                    .iter()
                    .flat_map(|w| {
                        w.sessions
                            .iter()
                            .map(|sess| sess.id.clone())
                            .chain(w.script_runs.iter().map(|run| run.id.clone()))
                    })
                    .collect::<std::collections::HashSet<_>>()
            })
            .await
    });

    for name in tmux::list_sessions(tmux_bin) {
        if known.contains(&name) {
            continue;
        }
        warn!(session = %name, "killing orphaned tmux session");
        tmux::kill_session(tmux_bin, &name);
    }
}

/// Build the default OS menu, then append Theme items under the View submenu.
/// If the layout of the default menu changes upstream and "View" isn't found,
/// the items are tucked into the app-name submenu as a fallback.
fn install_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    let load = MenuItem::with_id(app, "theme_load", "Load .itermcolors…", true, None::<&str>)?;
    let reset = MenuItem::with_id(app, "theme_reset", "Reset theme to default", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let items: &[&dyn IsMenuItem<_>] = &[&sep, &load, &reset];

    let mut appended = false;
    for kind in menu.items()? {
        if let Some(sub) = kind.as_submenu() {
            if sub.text().unwrap_or_default() == "View" {
                sub.append_items(items)?;
                appended = true;
                break;
            }
        }
    }
    if !appended {
        warn!("View submenu not found; theme items not installed");
    }
    menu.set_as_app_menu()?;
    Ok(())
}

fn handle_theme_load(app: &tauri::AppHandle) {
    let app = app.clone();
    app.clone()
        .dialog()
        .file()
        .add_filter("iTerm2 colors", &["itermcolors"])
        .pick_file(move |picked| {
            let Some(picked) = picked else { return };
            let Ok(source) = picked.into_path() else { return };
            let save_path = app.state::<Paths>().theme_file();
            if let Err(e) = theme::load_and_emit(&app, &source, &save_path) {
                error!(error = %e, source = %source.display(), "load theme failed");
            }
        });
}

fn handle_theme_reset(app: &tauri::AppHandle) {
    let save_path = app.state::<Paths>().theme_file();
    if let Err(e) = theme::clear_and_emit(app, &save_path) {
        error!(error = %e, "clear theme failed");
    }
}
