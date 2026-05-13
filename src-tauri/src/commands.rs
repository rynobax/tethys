use std::sync::Arc;

use chrono::Utc;
use serde::Deserialize;
use tauri::{ipc::Channel, AppHandle, Emitter, State};
use tracing::{info, warn};

use std::path::{Path, PathBuf};

use tauri::ipc::InvokeResponseBody;

use crate::claude;
use crate::claude_local;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::github::poller::{AuthSnapshot, GithubPoller};
use crate::inprogress::InProgressWorkspaces;
use crate::job::{JobEvent, JobTx};
use crate::paths::Paths;
use crate::purge::Purger;
use crate::reconcile::{self, Discrepancies};
use crate::registry::{self, starter_template, RegistryLoad, Repo};
use crate::sessions::{SessionInfo, SessionSupervisor};
use crate::setup;
use crate::state::{
    ClaudeSessionMeta, RepoLink, SystemErrorEntry, Workspace, WorkspaceId, WorkspaceStatus,
};
use crate::dev_orchestrator::{self, BeMode, OrchestratorConfig};
use crate::dev_servers::{self, DevServerLocks, DevStateSnapshot};
use crate::store::Store;
use crate::theme::Theme;
use crate::tmux::{self, TmuxBin};

#[tauri::command]
pub async fn list_workspaces(store: State<'_, Arc<Store>>) -> AppResult<Vec<Workspace>> {
    Ok(store.read(|s| s.workspaces.clone()).await)
}

#[tauri::command]
pub async fn get_workspace(
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<Workspace> {
    store
        .read(|s| s.find_workspace(&id).cloned())
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))
}

#[tauri::command]
pub fn list_repos(registry: State<'_, Arc<RegistryLoad>>) -> AppResult<Vec<Repo>> {
    let reg = registry.require()?;
    Ok(reg.repos.clone())
}

#[tauri::command]
pub fn registry_status(registry: State<'_, Arc<RegistryLoad>>) -> RegistryLoad {
    (**registry).clone()
}

#[tauri::command]
pub async fn github_auth_status(
    poller: State<'_, Arc<GithubPoller>>,
) -> AppResult<AuthSnapshot> {
    Ok(poller.auth_snapshot().await)
}

#[tauri::command]
pub async fn github_reprobe_auth(
    poller: State<'_, Arc<GithubPoller>>,
) -> AppResult<AuthSnapshot> {
    poller.probe_login().await;
    Ok(poller.auth_snapshot().await)
}

#[tauri::command]
pub fn open_repos_config(paths: State<'_, Paths>) -> AppResult<()> {
    let path = paths.repos_config_file();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, starter_template())?;
        info!(?path, "wrote starter repos.toml");
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .status()
            .map_err(|e| AppError::Other(format!("failed to open {}: {e}", path.display())))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        return Err(AppError::Other(
            "open_repos_config is only implemented for macOS in M2".into(),
        ));
    }

    Ok(())
}

#[tauri::command]
pub async fn open_in_vscode(
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
    let workspace_root: PathBuf = store
        .read(|s| {
            s.find_workspace(&id).and_then(|w| {
                w.repo_links
                    .first()
                    .and_then(|r| r.worktree_path.parent().map(|p| p.to_path_buf()))
            })
        })
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;

    std::process::Command::new("open")
        .args(["-a", "Visual Studio Code"])
        .arg(&workspace_root)
        .status()
        .map_err(|e| {
            AppError::Other(format!(
                "failed to open {} in VS Code: {e}",
                workspace_root.display()
            ))
        })?;

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceArgs {
    /// Frontend-minted UUID. Lets us insert a `Creating` draft into state
    /// immediately, so the sidebar row appears in its final position from
    /// the moment the user clicks Create — no later reorder, no parallel
    /// "pending" concept.
    pub workspace_id: WorkspaceId,
    pub branch: String,
    pub repo_selections: Vec<String>,
    /// Optional alternate entry-point binary name (e.g. `claude-hipaa`).
    /// Resolved on the login-shell PATH at spawn time.
    #[serde(default)]
    pub claude_binary: Option<String>,
}

struct RepoProvision<'a> {
    repo: &'a Repo,
    worktree_path: &'a Path,
    branch: &'a str,
    paths: &'a Paths,
    tx: &'a JobTx,
}

/// Clone (if needed) → pull → branch pre-check → worktree add → install
/// `.claude/settings.local.json` symlink → run setup script. Returns the
/// `RepoLink` to push into state. Caller is responsible for teardown on
/// later failure (we don't know whether sibling repos still need to be
/// provisioned after us).
async fn provision_repo_worktree(ctx: RepoProvision<'_>) -> AppResult<RepoLink> {
    let clone_path = ctx.paths.repo_clone_path(&ctx.repo.key);

    git::ensure_clone(&clone_path, &ctx.repo.remote_url, ctx.tx, &ctx.repo.key).await?;
    git::pull_clone(&clone_path, ctx.tx, &ctx.repo.key).await?;

    // Pre-check: if the branch already exists, git worktree add will fail
    // with a fatal. We bail here with a clearer message — and (for the
    // multi-repo create flow) avoid partially-creating worktrees in other
    // repos first.
    if git::branch_exists(&clone_path, ctx.branch).await? {
        return Err(AppError::Other(format!(
            "branch '{}' already exists in {}. Pick a different branch name, \
             or delete the stale branch first.",
            ctx.branch, ctx.repo.key
        )));
    }

    // If the branch already exists on the remote, create the local branch
    // tracking it instead of branching off HEAD — saves the manual
    // upstream-set + reset dance.
    let track_from = if git::remote_branch_exists(&clone_path, "origin", ctx.branch).await? {
        Some(format!("origin/{}", ctx.branch))
    } else {
        None
    };

    git::worktree_add(
        &clone_path,
        ctx.worktree_path,
        ctx.branch,
        track_from.as_deref(),
        ctx.tx,
        &ctx.repo.key,
    )
    .await?;

    claude_local::install_symlink(
        ctx.worktree_path,
        &ctx.paths.repo_shared_claude_local(&ctx.repo.key),
        ctx.tx,
        &ctx.repo.key,
    )
    .await?;

    copy_env_from_clone(&clone_path, ctx.worktree_path, ctx.tx, &ctx.repo.key).await?;

    let mut link = RepoLink {
        repo_key: ctx.repo.key.clone(),
        worktree_path: ctx.worktree_path.to_path_buf(),
        setup_script_ran_at: None,
        github: None,
    };

    if let Some(script) = ctx
        .repo
        .default_setup_script
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        // Pre-warm node_modules from the base clone via APFS clonefile so the
        // setup script (yarn/pnpm/npm install) only has to reconcile drift
        // instead of installing the whole tree from scratch. The setup_warmer
        // background task keeps the base clone's node_modules current.
        setup::warm_node_modules_from_clone(
            &clone_path,
            ctx.worktree_path,
            ctx.tx,
            &ctx.repo.key,
        )
        .await;

        setup::run_setup_script(
            script,
            ctx.worktree_path,
            ctx.repo.setup_timeout_secs,
            ctx.tx,
            &ctx.repo.key,
        )
        .await?;
        link.setup_script_ran_at = Some(Utc::now());
    }

    Ok(link)
}

/// Copy `<clone_path>/.env` into the new worktree if it exists. `.env` is
/// gitignored in most repos, so `git worktree add` won't carry it over —
/// but setup scripts and dev servers usually need it. Missing source is a
/// silent no-op; an existing `.env` in the worktree is left alone.
async fn copy_env_from_clone(
    clone_path: &Path,
    worktree_path: &Path,
    tx: &JobTx,
    repo_key: &str,
) -> AppResult<()> {
    let src = clone_path.join(".env");
    match tokio::fs::symlink_metadata(&src).await {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::Io(e)),
    }

    let dst = worktree_path.join(".env");
    if tokio::fs::try_exists(&dst).await? {
        return Ok(());
    }

    tokio::fs::copy(&src, &dst).await?;
    tx.status("copied .env from clone", Some(repo_key));
    Ok(())
}

struct RepoTeardown<'a> {
    repo_key: &'a str,
    worktree_path: &'a Path,
    branch: &'a str,
    paths: &'a Paths,
    tx: &'a JobTx,
}

/// Best-effort reverse of `provision_repo_worktree`: force-remove the
/// worktree, prune stale registrations, delete the branch we created.
/// Errors are streamed as status events but never bubbled — teardown is
/// always best-effort.
async fn teardown_repo_worktree(ctx: RepoTeardown<'_>) {
    if !ctx.worktree_path.exists() {
        return;
    }
    let clone_path = ctx.paths.repo_clone_path(ctx.repo_key);
    if let Err(cleanup_err) =
        git::worktree_remove(&clone_path, ctx.worktree_path, true, ctx.tx, ctx.repo_key).await
    {
        ctx.tx.status(
            format!("cleanup failed for {}: {cleanup_err}", ctx.repo_key),
            Some(ctx.repo_key),
        );
    }
    git::worktree_prune_best_effort(&clone_path, ctx.tx, ctx.repo_key).await;
    git::branch_delete_best_effort(&clone_path, ctx.branch, ctx.tx, ctx.repo_key).await;
}

/// Orchestrates clone + worktree add + setup script for every selected repo,
/// streaming progress to the frontend via `on_event`.
///
/// The workspace lands in `AppState` as `Creating` *before* any I/O so the
/// sidebar row appears at its final position from t=0; on success it flips
/// to `Ready`, on failure to `CreationFailed { error }` (and the worktrees
/// get torn down). The boot-time prune in `Store::load` clears any non-Ready
/// entries left by a crashed run.
#[tauri::command]
pub async fn create_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    paths: State<'_, Paths>,
    in_progress: State<'_, InProgressWorkspaces>,
    args: CreateWorkspaceArgs,
    on_event: Channel<JobEvent>,
) -> AppResult<Workspace> {
    let id = args.workspace_id.trim().to_string();
    if id.is_empty() {
        return Err(AppError::Other("workspace_id is required".into()));
    }
    let branch = args.branch.trim().to_string();
    if branch.is_empty() {
        return Err(AppError::Other("branch is required".into()));
    }
    if args.repo_selections.is_empty() {
        return Err(AppError::Other(
            "pick at least one repo to include in the workspace".into(),
        ));
    }
    let claude_binary = args
        .claude_binary
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // Validate up-front so the user finds out before we clone repos.
    if let Some(bin) = claude_binary.as_deref() {
        claude::resolve_named(bin)?;
    }

    let reg = registry.require()?;
    let selected: Vec<Repo> = args
        .repo_selections
        .iter()
        .map(|k| {
            reg.find_repo(k)
                .cloned()
                .ok_or_else(|| AppError::Other(format!("unknown repo key: {k}")))
        })
        .collect::<AppResult<Vec<_>>>()?;

    let workspace_dir = registry::sanitize_branch_for_dir(&branch);
    // Block collisions before we start cloning/fetching. Two workspaces with
    // the same branch on different repo sets would otherwise share a parent
    // dir, and deleting one would clobber the other on the `rm -rf` step.
    let workspace_root = reg.worktree_root.join(&workspace_dir);
    if workspace_root.exists() {
        return Err(AppError::Other(format!(
            "a worktree directory already exists at {}. Pick a different \
             branch name, or remove the existing directory first.",
            workspace_root.display()
        )));
    }

    // Insert the draft now so the sidebar row exists for the entire
    // provisioning lifetime. `status=Creating` drives the spinner UI; later
    // mutations flip it to `Ready` or `CreationFailed` in place — id and
    // position never change.
    let draft = Workspace {
        id: id.clone(),
        branch: branch.clone(),
        created_at: Utc::now(),
        repo_links: Vec::new(),
        sessions: Vec::new(),
        claude_binary: claude_binary.clone(),
        deleted_at: None,
        archived_at: None,
        status: WorkspaceStatus::Creating,
        session_order: None,
        dev_servers: None,
    };
    store
        .mutate(|s| {
            if s.workspaces.iter().any(|w| w.id == draft.id) {
                return Err(AppError::Other(format!(
                    "workspace_id collision: {} is already in state",
                    draft.id
                )));
            }
            s.workspaces.insert(0, draft.clone());
            Ok(())
        })
        .await?;
    emit_workspace_changed(&app, &id);

    // Register as in-progress so the reconciler doesn't flag our worktree
    // dirs as orphans mid-create. Guard removes the entry on drop — normal
    // return, `?`, panic, or task cancellation.
    let _in_progress_guard = in_progress.insert(workspace_dir.clone());
    let tx = spawn_event_forwarder(on_event);

    let orchestrate = async {
        let mut created: Vec<RepoLink> = Vec::new();
        for repo in &selected {
            let worktree_path = reg.plan_worktree_path(&workspace_dir, &repo.key);
            let link = provision_repo_worktree(RepoProvision {
                repo,
                worktree_path: &worktree_path,
                branch: &branch,
                paths: &paths,
                tx: &tx,
            })
            .await?;
            created.push(link);
        }
        Ok::<_, AppError>(created)
    };

    match orchestrate.await {
        Ok(created_links) => {
            let stored = store
                .mutate(|s| {
                    let ws = s
                        .find_workspace_mut(&id)
                        .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;
                    ws.repo_links = created_links;
                    ws.status = WorkspaceStatus::Ready;
                    Ok(ws.clone())
                })
                .await?;

            regen_workspace_root_settings(&stored, &paths, &tx).await;

            info!(id = %stored.id, branch = %stored.branch, repos = stored.repo_links.len(), "created workspace");
            let _ = tx.0.send(JobEvent::Success);
            emit_workspace_changed(&app, &stored.id);
            Ok(stored)
        }
        Err(e) => {
            let msg = e.to_string();
            warn!(error = %msg, "workspace create failed; rolling back worktrees");
            tx.status(format!("tearing down partial workspace: {msg}"), None);

            // Best-effort teardown of anything we managed to create. The
            // branch pre-check inside provision_repo_worktree guarantees we
            // didn't inherit any pre-existing branches, so deleting them
            // here is safe.
            for repo in selected.iter().rev() {
                let worktree_path = reg.plan_worktree_path(&workspace_dir, &repo.key);
                teardown_repo_worktree(RepoTeardown {
                    repo_key: &repo.key,
                    worktree_path: &worktree_path,
                    branch: &branch,
                    paths: &paths,
                    tx: &tx,
                })
                .await;
            }

            // Remove the now-empty parent dir so the reconciler doesn't
            // flag it as an orphan on the next tick.
            let parent = reg.worktree_root.join(&workspace_dir);
            if parent.exists() && reconcile::is_under(&reg.worktree_root, &parent) {
                if let Err(e) = tokio::fs::remove_dir_all(&parent).await {
                    warn!(path = %parent.display(), error = %e, "failed to remove partial workspace dir");
                }
            }

            // Flip the draft to CreationFailed so the row stays put with the
            // error visible in the detail pane. The user dismisses via the
            // existing `forget_workspace` command.
            let mutate_result = store
                .mutate(|s| {
                    if let Some(ws) = s.find_workspace_mut(&id) {
                        ws.status = WorkspaceStatus::CreationFailed {
                            error: msg.clone(),
                        };
                    }
                    Ok(())
                })
                .await;
            if let Err(mutate_err) = mutate_result {
                warn!(error = %mutate_err, "failed to mark workspace as CreationFailed");
            }
            emit_workspace_changed(&app, &id);

            let _ = tx.0.send(JobEvent::Failed { error: msg });
            Err(e)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AddRepoArgs {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
}

/// Add another repo's worktree to an existing workspace on the workspace's
/// branch. Mirrors a single-repo iteration of `create_workspace`: clone +
/// branch pre-check + worktree add + claude_local symlink + setup script,
/// then push the new `RepoLink` into state on success. On failure, tears
/// down only the worktree it created — leaves the rest of the workspace
/// intact.
#[tauri::command]
pub async fn add_repo_to_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    paths: State<'_, Paths>,
    args: AddRepoArgs,
    on_event: Channel<JobEvent>,
) -> AppResult<Workspace> {
    let reg = registry.require()?;
    let repo = reg
        .find_repo(&args.repo_key)
        .cloned()
        .ok_or_else(|| AppError::Other(format!("unknown repo key: {}", args.repo_key)))?;

    let (branch, already_present, is_deleted) = store
        .read(|s| {
            s.find_workspace(&args.workspace_id).map(|w| {
                (
                    w.branch.clone(),
                    w.repo_links.iter().any(|r| r.repo_key == args.repo_key),
                    w.deleted_at.is_some(),
                )
            })
        })
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;

    if is_deleted {
        return Err(AppError::Other(
            "workspace is soft-deleted; cancel deletion before adding repos".into(),
        ));
    }
    if already_present {
        return Err(AppError::Other(format!(
            "repo '{}' is already in this workspace",
            args.repo_key
        )));
    }

    let workspace_dir = registry::sanitize_branch_for_dir(&branch);
    let worktree_path = reg.plan_worktree_path(&workspace_dir, &repo.key);

    if worktree_path.exists() {
        return Err(AppError::Other(format!(
            "a worktree directory already exists at {}. Remove it first or \
             pick a different repo.",
            worktree_path.display()
        )));
    }

    let tx = spawn_event_forwarder(on_event);

    let provision = provision_repo_worktree(RepoProvision {
        repo: &repo,
        worktree_path: &worktree_path,
        branch: &branch,
        paths: &paths,
        tx: &tx,
    })
    .await;

    match provision {
        Ok(link) => {
            let updated = store
                .mutate(|s| {
                    let ws = s
                        .find_workspace_mut(&args.workspace_id)
                        .ok_or_else(|| {
                            AppError::WorkspaceNotFound(args.workspace_id.clone())
                        })?;
                    if ws.repo_links.iter().any(|r| r.repo_key == link.repo_key) {
                        return Err(AppError::Other(format!(
                            "repo '{}' is already in this workspace",
                            link.repo_key
                        )));
                    }
                    ws.repo_links.push(link.clone());
                    Ok(ws.clone())
                })
                .await?;

            regen_workspace_root_settings(&updated, &paths, &tx).await;

            info!(
                id = %args.workspace_id,
                repo = %args.repo_key,
                branch = %branch,
                "added repo to workspace"
            );
            let _ = tx.0.send(JobEvent::Success);
            emit_workspace_changed(&app, &args.workspace_id);
            Ok(updated)
        }
        Err(e) => {
            let msg = e.to_string();
            warn!(error = %msg, "add_repo_to_workspace failed; rolling back worktree");
            tx.status(format!("rolling back: {msg}"), None);
            teardown_repo_worktree(RepoTeardown {
                repo_key: &repo.key,
                worktree_path: &worktree_path,
                branch: &branch,
                paths: &paths,
                tx: &tx,
            })
            .await;
            let _ = tx.0.send(JobEvent::Failed { error: msg });
            Err(e)
        }
    }
}

/// Soft delete: mark the workspace as deleted and kill any live PTY sessions
/// so they can't keep writing to a worktree we're about to tear down. The
/// hourly purger does the actual git/worktree cleanup once the entry is
/// older than the grace window. Use `cancel_delete_workspace` to undo
/// before the purger runs.
#[tauri::command]
pub async fn delete_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    tmux_bin: State<'_, TmuxBin>,
    id: WorkspaceId,
) -> AppResult<()> {
    let session_ids: Vec<String> = store
        .read(|s| {
            s.find_workspace(&id)
                .map(|w| w.sessions.iter().map(|m| m.id.clone()).collect())
        })
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;

    // Kill tmux sessions so claude processes stop writing to the worktree
    // before the purger removes it. The supervisor reacts to the resulting
    // session:exit and cleans up its own state.
    if !tmux_bin.0.as_os_str().is_empty() {
        for sid in &session_ids {
            tmux::kill_session(&tmux_bin.0, sid);
        }
    }

    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&id)
                .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;
            // Idempotent: re-deleting an already-soft-deleted workspace
            // refreshes the timestamp, which extends the grace window.
            ws.deleted_at = Some(Utc::now());
            // Archive + delete are mutually exclusive views; clear archive
            // so the entry doesn't double-count if someone unarchives later.
            ws.archived_at = None;
            Ok(())
        })
        .await?;

    info!(%id, "soft-deleted workspace");
    emit_workspace_changed(&app, &id);
    let _ = app.emit("system_status:changed", &());
    Ok(())
}

/// Undo a soft delete. Only succeeds if the purger hasn't already
/// reaped the workspace.
#[tauri::command]
pub async fn cancel_delete_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&id)
                .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;
            ws.deleted_at = None;
            Ok(())
        })
        .await?;
    emit_workspace_changed(&app, &id);
    let _ = app.emit("system_status:changed", &());
    Ok(())
}

#[tauri::command]
pub async fn archive_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&id)
                .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;
            ws.archived_at = Some(Utc::now());
            Ok(())
        })
        .await?;
    emit_workspace_changed(&app, &id);
    Ok(())
}

#[tauri::command]
pub async fn unarchive_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&id)
                .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;
            ws.archived_at = None;
            Ok(())
        })
        .await?;
    emit_workspace_changed(&app, &id);
    Ok(())
}

/// Reorder the active workspaces (everything not soft-deleted and not
/// archived). The frontend computes a new ordering by drag-and-drop and
/// posts the resulting ID list. Workspaces not in the list keep their
/// current relative position in `AppState.workspaces`.
#[tauri::command]
pub async fn reorder_workspaces(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    ids: Vec<WorkspaceId>,
) -> AppResult<()> {
    store
        .mutate(|s| {
            // Validate every id exists; bail without mutating on mismatch
            // so a stale frontend snapshot can't shuffle the wrong rows.
            for id in &ids {
                if !s.workspaces.iter().any(|w| &w.id == id) {
                    return Err(AppError::WorkspaceNotFound(id.clone()));
                }
            }
            // Pull the named workspaces out in their requested order.
            let mut moved: Vec<Workspace> = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(pos) = s.workspaces.iter().position(|w| &w.id == id) {
                    moved.push(s.workspaces.remove(pos));
                }
            }
            // Re-insert at the front. Archived/soft-deleted entries that
            // weren't included keep their positions after the moved block.
            for ws in moved.into_iter().rev() {
                s.workspaces.insert(0, ws);
            }
            Ok(())
        })
        .await?;
    let _ = app.emit("workspace:reordered", &());
    Ok(())
}

/// Trigger the background purger immediately. Used by the "Run cleanup
/// now" button on the system status page. Still respects the 1-hour
/// grace window — entries deleted under an hour ago stay put.
#[tauri::command]
pub fn run_purge_now(purger: State<'_, Arc<Purger>>) -> AppResult<()> {
    purger.request_tick();
    Ok(())
}

#[tauri::command]
pub async fn list_system_errors(
    store: State<'_, Arc<Store>>,
) -> AppResult<Vec<SystemErrorEntry>> {
    Ok(store.read(|s| s.system_errors.clone()).await)
}

#[tauri::command]
pub async fn dismiss_system_error(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    id: String,
) -> AppResult<()> {
    store
        .mutate(|s| {
            s.system_errors.retain(|e| e.id != id);
            Ok(())
        })
        .await?;
    let _ = app.emit("system_status:changed", &());
    Ok(())
}

#[tauri::command]
pub async fn list_discrepancies(
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    in_progress: State<'_, InProgressWorkspaces>,
) -> AppResult<Discrepancies> {
    let snapshot = store.read(|s| s.clone()).await;
    let pending = in_progress.snapshot();
    let reg = match &**registry {
        RegistryLoad::Ok { registry, .. } => Some(registry),
        _ => None,
    };
    Ok(reconcile::scan(&snapshot, reg, &pending).await)
}

/// Delete a directory that the reconciler flagged as orphaned. The path is
/// validated against `worktree_root` to block traversal-style misuse.
#[tauri::command]
pub async fn remove_orphan_dir(
    registry: State<'_, Arc<RegistryLoad>>,
    path: PathBuf,
) -> AppResult<()> {
    let reg = registry.require()?;
    if !reconcile::is_under(&reg.worktree_root, &path) {
        return Err(AppError::Other(format!(
            "refusing to remove {}: not under worktree_root",
            path.display()
        )));
    }
    tokio::fs::remove_dir_all(&path).await?;
    info!(?path, "removed orphaned worktree dir");
    Ok(())
}

/// Drop a workspace from state without running any git ops. Used when a
/// workspace's worktrees are all missing and the user just wants the row
/// gone.
#[tauri::command]
pub async fn forget_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    tmux_bin: State<'_, TmuxBin>,
    id: WorkspaceId,
) -> AppResult<()> {
    let session_ids: Vec<String> = store
        .read(|s| {
            s.find_workspace(&id)
                .map(|w| w.sessions.iter().map(|m| m.id.clone()).collect())
                .unwrap_or_default()
        })
        .await;

    let removed = store
        .mutate(|s| {
            let before = s.workspaces.len();
            s.workspaces.retain(|w| w.id != id);
            Ok(s.workspaces.len() < before)
        })
        .await?;
    if !removed {
        return Err(AppError::WorkspaceNotFound(id));
    }

    // State is gone — kill the tmux sessions too so they don't become
    // orphans reaped on the next boot.
    if !tmux_bin.0.as_os_str().is_empty() {
        for sid in &session_ids {
            tmux::kill_session(&tmux_bin.0, sid);
        }
    }

    info!(%id, "forgot workspace (state-only removal)");
    emit_workspace_changed(&app, &id);
    Ok(())
}

#[tauri::command]
pub fn list_sessions(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    workspace_id: WorkspaceId,
) -> Vec<SessionInfo> {
    supervisor.list_for_workspace(&workspace_id)
}

#[tauri::command]
pub async fn acknowledge_session_turn(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    workspace_id: WorkspaceId,
    session_id: String,
) -> AppResult<()> {
    supervisor
        .acknowledge_turn(&session_id, &workspace_id)
        .await
}

#[derive(Debug, serde::Deserialize)]
pub struct StartClaudeArgs {
    pub workspace_id: WorkspaceId,
    /// `None` => start the session at the workspace root (the parent dir
    /// containing each repo's worktree subdir).
    #[serde(default)]
    pub repo_key: Option<String>,
}

/// Spawn a fresh `claude` session in the given workspace/repo worktree.
/// Also writes a `ClaudeSessionMeta` into state with `claude_session_id`
/// left as `None` — it gets filled in by the `SessionStart` hook.
#[tauri::command]
pub async fn start_claude_session(
    app: AppHandle,
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    claude_bin: State<'_, ClaudeBin>,
    tmux_bin: State<'_, TmuxBin>,
    args: StartClaudeArgs,
) -> AppResult<SessionInfo> {
    spawn_claude(
        &app,
        &supervisor,
        &store,
        &claude_bin,
        &tmux_bin,
        &args,
        None,
    )
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct ResumeClaudeArgs {
    pub workspace_id: WorkspaceId,
    /// `None` matches a workspace-root session.
    #[serde(default)]
    pub repo_key: Option<String>,
    /// The `id` field from an existing `ClaudeSessionMeta` — its
    /// `claude_session_id` will be passed to `claude --resume`.
    pub session_meta_id: String,
}

#[tauri::command]
pub async fn resume_claude_session(
    app: AppHandle,
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    claude_bin: State<'_, ClaudeBin>,
    tmux_bin: State<'_, TmuxBin>,
    args: ResumeClaudeArgs,
) -> AppResult<SessionInfo> {
    // Pull claude_session_id + cwd from the ClaudeSessionMeta we already
    // persisted on the previous run.
    let (claude_sid, cwd) = store
        .read(|s| {
            s.find_workspace(&args.workspace_id).and_then(|w| {
                w.sessions
                    .iter()
                    .find(|sess| sess.id == args.session_meta_id)
                    .map(|sess| (sess.claude_session_id.clone(), sess.cwd.clone()))
            })
        })
        .await
        .ok_or_else(|| {
            AppError::Other(format!(
                "no session {} in workspace {}",
                args.session_meta_id, args.workspace_id
            ))
        })?;

    // If the tmux session from a prior run is still alive, reattach to it
    // — no claude respawn, no transcript replay. The Tethys SessionId is
    // the tmux session name, so we can probe directly.
    if !tmux_bin.0.as_os_str().is_empty()
        && tmux::has_session(&tmux_bin.0, &args.session_meta_id)
    {
        info!(
            session_id = %args.session_meta_id,
            "reattaching to live tmux session"
        );
        let info = supervisor.reattach_tmux(
            args.session_meta_id,
            args.workspace_id.clone(),
            args.repo_key,
            &cwd,
            &tmux_bin.0,
        )?;
        emit_workspace_changed(&app, &args.workspace_id);
        return Ok(info);
    }

    let claude_sid = claude_sid.ok_or_else(|| {
        AppError::Other(
            "session has no claude_session_id yet — resume not possible".into(),
        )
    })?;

    let start = StartClaudeArgs {
        workspace_id: args.workspace_id,
        repo_key: args.repo_key,
    };
    spawn_claude(
        &app,
        &supervisor,
        &store,
        &claude_bin,
        &tmux_bin,
        &start,
        Some(&claude_sid),
    )
    .await
}

async fn spawn_claude(
    app: &AppHandle,
    supervisor: &Arc<SessionSupervisor>,
    store: &Arc<Store>,
    claude_bin: &ClaudeBin,
    tmux_bin: &TmuxBin,
    args: &StartClaudeArgs,
    resume_claude_sid: Option<&str>,
) -> AppResult<SessionInfo> {
    if tmux_bin.0.as_os_str().is_empty() {
        return Err(AppError::Other(
            "tmux not found — install with `brew install tmux` and restart Tethys".into(),
        ));
    }

    // Resolve the cwd: a specific repo's worktree, or — when repo_key is
    // None — the workspace root (parent of every repo worktree).
    // Also pull the per-workspace claude binary override, if any.
    let (cwd, ws_binary) = store
        .read(|s| {
            let w = s.find_workspace(&args.workspace_id)?;
            let cwd = match args.repo_key.as_deref() {
                Some(key) => w
                    .repo_links
                    .iter()
                    .find(|r| r.repo_key == key)
                    .map(|r| r.worktree_path.clone()),
                None => w
                    .repo_links
                    .first()
                    .and_then(|r| r.worktree_path.parent().map(|p| p.to_path_buf())),
            }?;
            Some((cwd, w.claude_binary.clone()))
        })
        .await
        .ok_or_else(|| {
            AppError::Other(match args.repo_key.as_deref() {
                Some(key) => format!(
                    "no worktree for {}/{} in state",
                    args.workspace_id, key
                ),
                None => format!(
                    "workspace {} has no repos — can't resolve a root dir",
                    args.workspace_id
                ),
            })
        })?;

    let resolved_bin = match ws_binary.as_deref() {
        Some(bin) => claude::resolve_named(bin)?,
        None => claude_bin.0.clone(),
    };

    let (info, _token) = supervisor.spawn_claude(
        args.workspace_id.clone(),
        args.repo_key.clone(),
        &cwd,
        &tmux_bin.0,
        &resolved_bin,
        resume_claude_sid,
    )?;

    // Persist a ClaudeSessionMeta entry so resume works across restarts.
    // claude_session_id is filled in by the SessionStart hook once it
    // arrives. We key on the Tethys-internal `id` (== SessionSupervisor id)
    // so the UI and supervisor use a shared identifier.
    let meta = ClaudeSessionMeta {
        id: info.id.clone(),
        kind: crate::state::SessionKind::Claude,
        repo_key: args.repo_key.clone(),
        cwd: cwd.clone(),
        claude_session_id: None,
        transcript_path: None,
        hidden: false,
        runtime_state: None,
        notification_type: None,
        turn_acknowledged: false,
        display_name: None,
    };

    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&args.workspace_id)
                .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;
            // Resuming? Drop the prior meta for this Claude conversation so
            // we don't accumulate dormant duplicates with the same
            // claude_session_id across runs.
            if let Some(csid) = resume_claude_sid {
                ws.sessions
                    .retain(|m| m.claude_session_id.as_deref() != Some(csid));
            }
            // Defensive: no dupes of the new tethys id either.
            ws.sessions.retain(|m| m.id != meta.id);
            ws.sessions.push(meta);
            Ok(())
        })
        .await?;

    emit_workspace_changed(app, &args.workspace_id);
    Ok(info)
}

#[derive(Debug, serde::Deserialize)]
pub struct SetClaudeHiddenArgs {
    pub workspace_id: WorkspaceId,
    pub session_id: String,
    pub hidden: bool,
}

/// Toggle a Claude session's `hidden` flag in state. Cosmetic only — the
/// tmux session and the supervisor's `SessionHandle` keep running.
#[tauri::command]
pub async fn set_claude_session_hidden(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    args: SetClaudeHiddenArgs,
) -> AppResult<()> {
    let touched = store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&args.workspace_id)
                .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;
            let Some(meta) = ws.sessions.iter_mut().find(|m| m.id == args.session_id) else {
                return Ok(false);
            };
            meta.hidden = args.hidden;
            Ok(true)
        })
        .await?;

    if !touched {
        return Err(AppError::Other(format!(
            "session {} not found in workspace {}",
            args.session_id, args.workspace_id
        )));
    }

    emit_workspace_changed(&app, &args.workspace_id);
    Ok(())
}

/// Newtype so `claude_bin` can be managed in Tauri state.
pub struct ClaudeBin(pub std::path::PathBuf);

/// Subscribe to live PTY bytes and return the current scrollback. The
/// channel carries raw bytes via `InvokeResponseBody::Raw` — no JSON
/// serialization overhead per chunk.
#[tauri::command]
pub fn attach_session(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    session_id: String,
    on_bytes: tauri::ipc::Channel<InvokeResponseBody>,
) -> AppResult<Vec<u8>> {
    supervisor.attach(&session_id, on_bytes)
}

#[tauri::command]
pub fn send_input(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    supervisor.send_input(&session_id, &data)?;
    // Turn state is driven by Claude Code's UserPromptSubmit / Stop /
    // Notification hooks — no optimistic flip needed here.
    Ok(())
}

#[tauri::command]
pub fn resize_session(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    supervisor.resize(&session_id, cols, rows)
}

#[tauri::command]
pub fn get_theme(paths: State<'_, Paths>) -> AppResult<Option<Theme>> {
    Theme::load_saved(&paths.theme_file())
}

/// Read file paths from the macOS general pasteboard. Used on Cmd+V when the
/// browser-side `clipboardData` only carries opaque `File` objects (no
/// `text/plain`, no `text/uri-list`) — WKWebView hides the real path. We need
/// it so paste-of-a-file inserts the path text iTerm2-style instead of relying
/// on WKWebView's hidden auto-insert (which always triggers Claude Code's
/// `[Image #N]` flow regardless of the actual file type).
#[tauri::command]
pub fn read_clipboard_file_paths() -> AppResult<Vec<String>> {
    const SCRIPT: &str = r#"ObjC.import('AppKit');
const pb = $.NSPasteboard.generalPasteboard;
const urls = pb.readObjectsForClassesOptions($.NSArray.arrayWithObject($.NSURL), $());
const paths = [];
if (!urls.isNil()) {
    for (let i = 0; i < urls.count; i++) {
        const u = urls.objectAtIndex(i);
        if (u.isFileURL) paths.push(ObjC.unwrap(u.path));
    }
}
JSON.stringify(paths);"#;

    let output = std::process::Command::new("osascript")
        .args(["-l", "JavaScript", "-e", SCRIPT])
        .output()?;
    if !output.status.success() {
        return Err(AppError::Other(format!(
            "osascript exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(serde_json::from_str(stdout.trim())?)
}

/// Regenerate `<workspace_root>/.claude/settings.local.json` from the
/// current set of repo links. Best-effort: failures are surfaced as a
/// status event but never fail the parent command.
async fn regen_workspace_root_settings(workspace: &Workspace, paths: &Paths, tx: &JobTx) {
    let Some(workspace_root) = workspace
        .repo_links
        .first()
        .and_then(|r| r.worktree_path.parent().map(|p| p.to_path_buf()))
    else {
        return;
    };
    let repo_keys: Vec<String> = workspace
        .repo_links
        .iter()
        .map(|r| r.repo_key.clone())
        .collect();
    if let Err(e) =
        claude_local::write_workspace_root_settings(&workspace_root, &repo_keys, paths).await
    {
        warn!(
            workspace = %workspace.id,
            error = %e,
            "failed to regenerate workspace-root settings.local.json"
        );
        tx.status(
            format!("workspace-root settings regen failed: {e}"),
            None,
        );
    }
}

fn emit_workspace_changed(app: &AppHandle, workspace_id: &str) {
    let _ = app.emit(
        "workspace:changed",
        serde_json::json!({ "workspace_id": workspace_id }),
    );
}

/// Spawn a task that drains an mpsc of `JobEvent` into the Tauri `Channel`.
/// Returns a `JobTx` the orchestrator uses to emit events. Dropping the tx
/// (or returning from the command) closes the mpsc and the forwarder exits.
fn spawn_event_forwarder(channel: Channel<JobEvent>) -> JobTx {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if channel.send(event).is_err() {
                break;
            }
        }
    });
    JobTx(tx)
}

// ── Session tab reorder + rename ─────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ReorderSessionsArgs {
    pub workspace_id: WorkspaceId,
    /// The full desired chip order, top/leftmost first. Caller passes
    /// the visible-on-screen order; the backend just persists it. Ids
    /// not in this list are appended on render in their existing order,
    /// so a partial list (e.g. only the user's pinned IDs) is also
    /// valid — but the UI sends the full order.
    pub session_ids: Vec<String>,
}

/// Commit a new session-chip order to the workspace. Persisted in
/// `Workspace.session_order`; `None` (un-set) means "fall back to the
/// default newest-first ordering".
#[tauri::command]
pub async fn reorder_sessions(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    args: ReorderSessionsArgs,
) -> AppResult<()> {
    let wid = args.workspace_id.clone();
    let order = args.session_ids;
    store
        .mutate(move |s| {
            let ws = s
                .find_workspace_mut(&wid)
                .ok_or_else(|| AppError::WorkspaceNotFound(wid.clone()))?;
            // De-duplicate while preserving order — the dnd-kit caller
            // shouldn't send dupes, but be defensive.
            let mut seen = std::collections::HashSet::new();
            let cleaned: Vec<String> = order
                .into_iter()
                .filter(|id| seen.insert(id.clone()))
                .collect();
            ws.session_order = if cleaned.is_empty() {
                None
            } else {
                Some(cleaned)
            };
            Ok(())
        })
        .await?;
    emit_workspace_changed(&app, &args.workspace_id);
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
pub struct RenameSessionArgs {
    pub workspace_id: WorkspaceId,
    pub session_id: String,
    /// `None` (or an empty/whitespace string, which we treat as None)
    /// clears the override and the chip falls back to its default label.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Set or clear the user's display name override for one session chip.
#[tauri::command]
pub async fn rename_session(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    args: RenameSessionArgs,
) -> AppResult<()> {
    let wid = args.workspace_id.clone();
    let sid = args.session_id.clone();
    let trimmed = args
        .display_name
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    store
        .mutate(move |s| {
            let ws = s
                .find_workspace_mut(&wid)
                .ok_or_else(|| AppError::WorkspaceNotFound(wid.clone()))?;
            let meta = ws
                .sessions
                .iter_mut()
                .find(|m| m.id == sid)
                .ok_or_else(|| {
                    AppError::Other(format!(
                        "no session {} in workspace {}",
                        sid, wid
                    ))
                })?;
            meta.display_name = trimmed;
            Ok(())
        })
        .await?;
    emit_workspace_changed(&app, &args.workspace_id);
    Ok(())
}

// ── Dev-server orchestration commands ─────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct StartDevServersArgs {
    pub workspace_id: WorkspaceId,
    /// Auto = git diff decides; ForceInclude = always start BE;
    /// ForceExclude = FE only. Defaults to Auto when missing.
    #[serde(default)]
    pub mode: BeMode,
}

#[tauri::command]
pub async fn start_dev_servers(
    app: AppHandle,
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    tmux_bin: State<'_, TmuxBin>,
    locks: State<'_, Arc<DevServerLocks>>,
    cfg: State<'_, Arc<OrchestratorConfig>>,
    args: StartDevServersArgs,
) -> AppResult<crate::state::DevServersMeta> {
    dev_servers::start(
        &app,
        supervisor.inner(),
        store.inner(),
        tmux_bin.inner(),
        locks.inner(),
        &args.workspace_id,
        args.mode,
        cfg.inner(),
    )
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct WorkspaceIdArg {
    pub workspace_id: WorkspaceId,
}

#[tauri::command]
pub async fn stop_dev_servers(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    tmux_bin: State<'_, TmuxBin>,
    locks: State<'_, Arc<DevServerLocks>>,
    cfg: State<'_, Arc<OrchestratorConfig>>,
    args: WorkspaceIdArg,
) -> AppResult<()> {
    dev_servers::stop(
        &app,
        store.inner(),
        tmux_bin.inner(),
        locks.inner(),
        &args.workspace_id,
        cfg.inner(),
    )
    .await
}

#[tauri::command]
pub async fn get_dev_state(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    args: WorkspaceIdArg,
) -> AppResult<DevStateSnapshot> {
    dev_servers::snapshot(supervisor.inner(), store.inner(), &args.workspace_id).await
}

#[tauri::command]
pub async fn detect_be_changes(
    store: State<'_, Arc<Store>>,
    cfg: State<'_, Arc<OrchestratorConfig>>,
    args: WorkspaceIdArg,
) -> AppResult<bool> {
    dev_servers::detect_be_changes(store.inner(), &args.workspace_id, cfg.inner()).await
}

/// Snapshot of memory pressure + per-workspace RAM. Cheap; the same
/// data the poller emits as `devserver:memory_updated` events. Useful
/// for the UI to fetch on mount before the first poller tick lands.
#[tauri::command]
pub async fn get_memory_snapshot(
    store: State<'_, Arc<Store>>,
    cfg: State<'_, Arc<OrchestratorConfig>>,
) -> AppResult<crate::memory_poller::MemorySnapshot> {
    Ok(crate::memory_poller::snapshot_now(store.inner(), cfg.inner()).await)
}
