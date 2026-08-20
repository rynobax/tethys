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
use crate::github::poller::{fetch_pr_status, AuthSnapshot, GithubPoller};
use crate::github::{
    parse_pr_reference, resolve_attach_target, GithubPrStatus, GithubSlug,
};
use crate::inprogress::InProgressWorkspaces;
use crate::job::{JobEvent, JobTx};
use crate::mcp::McpLaunch;
use crate::paths::Paths;
use crate::provision::{
    provision_repo_worktree, provision_workspace, teardown_repo_worktree, RepoProvision,
    RepoTeardown, WorkspaceProvision,
};
use crate::purge::Purger;
use crate::reconcile::{self, Discrepancies};
use crate::registry::{self, starter_template, RegistryLoad, Repo, RepoRegistry};
use crate::scripts::{ScriptInfo, ScriptSupervisor};
use crate::sessions::{start_session, SessionInfo, SessionSupervisor, StartSession};
use crate::state::{
    AttachedPr, Origin, ScriptRunMeta, SystemErrorEntry, Workspace, WorkspaceId,
};
use crate::store::Store;
use crate::theme::Theme;
use crate::tmux::{self, TmuxBin};
use crate::workspace_doc;

#[tauri::command]
pub async fn list_workspaces(store: State<'_, Arc<Store>>) -> AppResult<Vec<Workspace>> {
    Ok(store.read(|s| s.workspaces.clone()).await)
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

#[derive(Debug, Deserialize)]
pub struct AttachPrArgs {
    pub workspace_id: WorkspaceId,
    /// `null` => infer the repo from the reference's `owner/repo`, or from the
    /// workspace's only GitHub-linked repo.
    #[serde(default)]
    pub repo_key: Option<String>,
    /// `123`, `#123`, `owner/repo#123`, or a full GitHub PR URL.
    pub reference: String,
}

/// Manually track an extra PR on a workspace's repo link.
///
/// The poller only ever discovers the PR for the workspace's own branch, so a
/// second branch cut inside the same worktree needs its PR attached by hand.
/// The status is fetched here rather than left to the next tick, so a typo'd
/// number fails loudly instead of parking an empty chip in the UI.
#[tauri::command]
pub async fn attach_pr(
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    args: AttachPrArgs,
) -> AppResult<GithubPrStatus> {
    let pr = parse_pr_reference(&args.reference).ok_or_else(|| {
        AppError::Other(format!(
            "couldn't read a PR number from \"{}\" — paste a PR URL or a number",
            args.reference.trim()
        ))
    })?;
    let reg = registry.require()?;

    let repo_keys: Vec<String> = store
        .read(|s| {
            s.find_workspace(&args.workspace_id)
                .map(|w| w.repo_links.iter().map(|r| r.repo_key.clone()).collect())
        })
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;
    // Only GitHub-backed repos are attachable — the rest have no slug to query.
    let mut candidates: Vec<(String, GithubSlug)> = Vec::new();
    for key in repo_keys {
        if let Some(slug) = reg.find_repo(&key).and_then(|r| r.github_slug.clone()) {
            candidates.push((key, slug));
        }
    }

    let (repo_key, slug) =
        resolve_attach_target(&candidates, args.repo_key.as_deref(), &pr)
            .map_err(|e| AppError::Other(e.to_string()))?;

    let status = fetch_pr_status(&slug, pr.number)
        .await
        .map_err(|e| {
            AppError::Other(format!(
                "couldn't fetch PR #{} from {}/{}: {e}",
                pr.number, slug.owner, slug.name
            ))
        })?
        .ok_or_else(|| {
            AppError::Other(format!(
                "{}/{} has no PR #{}",
                slug.owner, slug.name, pr.number
            ))
        })?;

    let stored = status.clone();
    let workspace_id = args.workspace_id.clone();
    store
        .update_workspace(&workspace_id, move |ws| {
            let link = ws.link_mut(&repo_key).ok_or_else(|| {
                AppError::Other(format!("workspace has no worktree for {repo_key}"))
            })?;
            if link.github.as_ref().is_some_and(|g| g.pr_number == pr.number) {
                return Err(AppError::Other(format!(
                    "PR #{} is already tracked as this workspace's branch PR",
                    pr.number
                )));
            }
            if link.attached_prs.iter().any(|a| a.number == pr.number) {
                return Err(AppError::Other(format!(
                    "PR #{} is already attached to {repo_key}",
                    pr.number
                )));
            }
            link.attached_prs.push(AttachedPr {
                number: pr.number,
                attached_at: Utc::now(),
                status: Some(stored),
            });
            Ok(())
        })
        .await?;

    Ok(status)
}

#[derive(Debug, Deserialize)]
pub struct DetachPrArgs {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub pr_number: u32,
}

/// Stop tracking a manually-attached PR. Nothing on GitHub is touched.
#[tauri::command]
pub async fn detach_pr(
    store: State<'_, Arc<Store>>,
    args: DetachPrArgs,
) -> AppResult<()> {
    store
        .update_workspace(&args.workspace_id, |ws| {
            let link = ws.link_mut(&args.repo_key).ok_or_else(|| {
                AppError::Other(format!("workspace has no worktree for {}", args.repo_key))
            })?;
            link.attached_prs.retain(|a| a.number != args.pr_number);
            Ok(())
        })
        .await?;
    Ok(())
}

/// The editor Tethys opens files and worktrees in. Centralized so switching
/// editors is a one-line change shared by every "open in editor" action.
const EDITOR_APP: &str = "Visual Studio Code";

/// Open `path` (a file or directory) in [`EDITOR_APP`] via macOS `open -a`.
fn open_in_editor(path: &Path) -> AppResult<()> {
    std::process::Command::new("open")
        .args(["-a", EDITOR_APP])
        .arg(path)
        .status()
        .map_err(|e| {
            AppError::Other(format!(
                "failed to open {} in {EDITOR_APP}: {e}",
                path.display()
            ))
        })?;
    Ok(())
}

/// The VS Code CLI shipped inside the app bundle. Preferred over `code` on
/// `PATH` because a bundled Tethys launched from Finder inherits a minimal
/// `PATH` that won't include the `/usr/local/bin/code` symlink.
const VSCODE_CLI_BUNDLED: &str =
    "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";

fn vscode_cli() -> &'static str {
    if Path::new(VSCODE_CLI_BUNDLED).exists() {
        VSCODE_CLI_BUNDLED
    } else {
        "code"
    }
}

/// Open a workspace root in VS Code, reusing the last active window instead of
/// spawning a new one.
///
/// `open -a` hands the path to VS Code as a document, which opens a fresh
/// window every time. That gets expensive fast: each worktree is a full
/// checkout with its own `node_modules`, so every extra window means another
/// independent extension-host / TS-server / lint-server stack with nothing
/// shared. `--reuse-window` keeps all workspaces in a single window, swapping
/// the folder rather than multiplying the tooling.
fn open_workspace_in_editor(path: &Path) -> AppResult<()> {
    let status = std::process::Command::new(vscode_cli())
        .arg("--reuse-window")
        .arg(path)
        .status()
        .map_err(|e| {
            AppError::Other(format!(
                "failed to open {} in {EDITOR_APP}: {e}",
                path.display()
            ))
        })?;

    if !status.success() {
        return Err(AppError::Other(format!(
            "{EDITOR_APP} exited {status} opening {}",
            path.display()
        )));
    }
    Ok(())
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

    open_in_editor(&path)
}

#[tauri::command]
pub async fn open_in_vscode(
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
    // A workspace that exists but has no repo links is a Creating draft or a
    // failed creation — it has no root on disk yet. That is a different thing
    // from "no such workspace", and saying so is the difference between the
    // user waiting and the user hunting for a bug.
    let workspace_root: PathBuf = store
        .with_workspace(&id, Workspace::root_buf)
        .await?
        .ok_or_else(|| {
            AppError::Other(
                "workspace has no repos yet — nothing to open".to_string(),
            )
        })?;

    open_workspace_in_editor(&workspace_root)
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

/// Validate the request, insert the `Creating` draft, then hand the actual
/// provisioning to [`provision_workspace`], streaming its progress to the
/// frontend via `on_event`.
#[tauri::command]
pub async fn create_workspace(
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

    let draft = Workspace::draft(id.clone(), branch.clone(), claude_binary, Origin::Ui);
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
    store.notify_changed(&id);

    let tx = spawn_event_forwarder(on_event);
    provision_workspace(WorkspaceProvision {
        workspace_id: &id,
        branch: &branch,
        workspace_dir: &workspace_dir,
        repos: &selected,
        registry: reg,
        paths: &paths,
        store: &store,
        in_progress: &in_progress,
        tx: &tx,
    })
    .await
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
        .with_workspace(&args.workspace_id, |w| {
            (
                w.branch.clone(),
                w.has_link(&args.repo_key),
                w.deleted_at.is_some(),
            )
        })
        .await?;

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
                .update_workspace(&args.workspace_id, |ws| {
                    // Re-check both pre-conditions, not just one: provisioning
                    // took minutes of git I/O and the user may have soft-deleted
                    // the workspace in the meantime. Pushing a link onto a
                    // deleted workspace hands the purger a worktree it doesn't
                    // know it owns.
                    if ws.deleted_at.is_some() {
                        return Err(AppError::Other(
                            "workspace was deleted while the repo was being provisioned"
                                .into(),
                        ));
                    }
                    if ws.has_link(&link.repo_key) {
                        return Err(AppError::Other(format!(
                            "repo '{}' is already in this workspace",
                            link.repo_key
                        )));
                    }
                    ws.repo_links.push(link.clone());
                    Ok(ws.clone())
                })
                .await?;

            append_repo_to_workspace_root_settings(
                &updated,
                &args.repo_key,
                &paths,
                &tx,
            )
            .await;
            // The new repo changes both the "checked out here" list and the
            // "available to add" list, so the whole doc is rewritten.
            regen_workspace_claude_md(&updated, reg, &paths, &tx).await;

            info!(
                id = %args.workspace_id,
                repo = %args.repo_key,
                branch = %branch,
                "added repo to workspace"
            );
            let _ = tx.0.send(JobEvent::Success);
            Ok(updated)
        }
        Err(e) => {
            let msg = e.to_string();
            warn!(error = %msg, "add_repo_to_workspace failed; rolling back worktree");
            tx.status(format!("rolling back: {msg}"), None);
            // `provision_repo_worktree` already self-cleans on failure (deleting
            // any branch it created). This is a backstop for a stray worktree;
            // `created_branch: false` ensures it never deletes a branch here.
            teardown_repo_worktree(RepoTeardown {
                repo_key: &repo.key,
                worktree_path: &worktree_path,
                branch: &branch,
                created_branch: false,
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
    script_supervisor: State<'_, Arc<ScriptSupervisor>>,
    id: WorkspaceId,
) -> AppResult<()> {
    let session_ids: Vec<String> = store
        .with_workspace(&id, |w| w.sessions.iter().map(|m| m.id.clone()).collect())
        .await?;

    // Kill running scripts (dev servers etc.) before anything else — these
    // would otherwise keep writing to the worktree we're about to tear down,
    // and a long-lived `yarn dev` blocks the purger's `rm -rf`.
    if !tmux_bin.0.as_os_str().is_empty() {
        script_supervisor.kill_for_workspace(&id, &tmux_bin.0);
    }

    // Kill tmux sessions so claude processes stop writing to the worktree
    // before the purger removes it. The supervisor reacts to the resulting
    // session:exit and cleans up its own state.
    if !tmux_bin.0.as_os_str().is_empty() {
        for sid in &session_ids {
            tmux::kill_session(&tmux_bin.0, sid);
        }
    }

    store
        .update_workspace(&id, |ws| {
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
        .update_workspace(&id, |ws| {
            ws.deleted_at = None;
            Ok(())
        })
        .await?;
    let _ = app.emit("system_status:changed", &());
    Ok(())
}

#[tauri::command]
pub async fn archive_workspace(
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
    store
        .update_workspace(&id, |ws| {
            ws.archived_at = Some(Utc::now());
            Ok(())
        })
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn unarchive_workspace(
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
    store
        .update_workspace(&id, |ws| {
            ws.archived_at = None;
            Ok(())
        })
        .await?;
    Ok(())
}

/// Reorder the active workspaces (everything not soft-deleted and not
/// archived). The frontend computes a new ordering by drag-and-drop and
/// posts the resulting ID list. Workspaces not in the list keep their
/// current relative position in `AppState.workspaces`.
#[tauri::command]
pub async fn reorder_workspaces(
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
    // No event: the frontend reorders optimistically so the dropped row
    // doesn't flicker, and re-broadcasting the order would undo that.
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
pub async fn list_pending_permissions(
    paths: State<'_, Paths>,
) -> AppResult<Vec<crate::pending_permissions::PendingPermission>> {
    let file = crate::pending_permissions::load_file(&paths.pending_permissions_file()).await?;
    Ok(file.entries)
}

#[derive(Debug, Deserialize)]
pub struct ApplyPendingArgs {
    pub id: String,
    pub target_repo_keys: Vec<String>,
}

#[tauri::command]
pub async fn apply_pending_permission(
    app: AppHandle,
    paths: State<'_, Paths>,
    args: ApplyPendingArgs,
) -> AppResult<()> {
    if args.target_repo_keys.is_empty() {
        return Err(AppError::Other(
            "apply_pending_permission: target_repo_keys is empty".into(),
        ));
    }
    crate::pending_permissions::apply_pending(&paths, &args.id, &args.target_repo_keys).await?;
    let _ = app.emit("pending_permissions:changed", &());
    Ok(())
}

#[tauri::command]
pub async fn dismiss_pending_permission(
    app: AppHandle,
    paths: State<'_, Paths>,
    id: String,
) -> AppResult<()> {
    crate::pending_permissions::dismiss_pending(&paths, &id).await?;
    let _ = app.emit("pending_permissions:changed", &());
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
    script_supervisor: State<'_, Arc<ScriptSupervisor>>,
    id: WorkspaceId,
) -> AppResult<()> {
    let session_ids: Vec<String> = store
        .read(|s| {
            s.find_workspace(&id)
                .map(|w| w.sessions.iter().map(|m| m.id.clone()).collect())
                .unwrap_or_default()
        })
        .await;

    if !tmux_bin.0.as_os_str().is_empty() {
        script_supervisor.kill_for_workspace(&id, &tmux_bin.0);
    }

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
        .await;
    Ok(())
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
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    claude_bin: State<'_, ClaudeBin>,
    tmux_bin: State<'_, TmuxBin>,
    mcp: State<'_, Option<McpLaunch>>,
    args: StartClaudeArgs,
) -> AppResult<SessionInfo> {
    start_session(StartSession {
        supervisor: &supervisor,
        store: &store,
        workspace_id: &args.workspace_id,
        repo_key: args.repo_key,
        claude_bin: &claude_bin.0,
        tmux_bin: &tmux_bin.0,
        mcp: mcp.inner().as_ref(),
        resume_claude_sid: None,
        session_binary: None,
        brief: None,
    })
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
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    claude_bin: State<'_, ClaudeBin>,
    tmux_bin: State<'_, TmuxBin>,
    mcp: State<'_, Option<McpLaunch>>,
    args: ResumeClaudeArgs,
) -> AppResult<SessionInfo> {
    // Pull claude_session_id + cwd + binary override from the
    // ClaudeSessionMeta we already persisted on the previous run.
    let (claude_sid, cwd, session_binary) = store
        .read(|s| {
            s.find_workspace(&args.workspace_id).and_then(|w| {
                w.sessions
                    .iter()
                    .find(|sess| sess.id == args.session_meta_id)
                    .map(|sess| {
                        (
                            sess.claude_session_id.clone(),
                            sess.cwd.clone(),
                            sess.claude_binary.clone(),
                        )
                    })
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
        store.notify_changed(&args.workspace_id);
        return Ok(info);
    }

    let claude_sid = claude_sid.ok_or_else(|| {
        AppError::Other(
            "session has no claude_session_id yet — resume not possible".into(),
        )
    })?;

    start_session(StartSession {
        supervisor: &supervisor,
        store: &store,
        workspace_id: &args.workspace_id,
        repo_key: args.repo_key,
        claude_bin: &claude_bin.0,
        tmux_bin: &tmux_bin.0,
        mcp: mcp.inner().as_ref(),
        resume_claude_sid: Some(&claude_sid),
        session_binary: session_binary.as_deref(),
        brief: None,
    })
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct SwitchClaudeBinaryArgs {
    pub workspace_id: WorkspaceId,
    /// The `id` field from an existing `ClaudeSessionMeta`.
    pub session_meta_id: String,
    /// Binary name to switch to (e.g. `claude`, `claude-hipaa`). Stored as a
    /// per-session override and used to relaunch the conversation.
    pub claude_binary: String,
}

/// Whether a claude conversation can be resumed via `--resume`. Claude reports
/// a `claude_session_id` at startup but only writes the transcript to disk once
/// there's actual conversation, so `--resume` on a brand-new (empty) session
/// fails with "No conversation found". A non-empty transcript file on disk is
/// the reliable signal that resume will succeed.
fn transcript_is_resumable(path: Option<&Path>) -> bool {
    path.and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Switch the entry-point binary for a chat. Kills the running tmux session
/// (so the resume path can't just reattach the old process) and relaunches
/// claude under the new binary. If the conversation has been persisted to
/// disk, it resumes via `--resume <claude_session_id>` to preserve history;
/// otherwise (a fresh chat with no messages yet) it starts a new session under
/// the new binary — there's nothing to resume.
#[tauri::command]
pub async fn switch_claude_binary(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    claude_bin: State<'_, ClaudeBin>,
    tmux_bin: State<'_, TmuxBin>,
    mcp: State<'_, Option<McpLaunch>>,
    args: SwitchClaudeBinaryArgs,
) -> AppResult<SessionInfo> {
    let binary = args.claude_binary.trim().to_string();
    if binary.is_empty() {
        return Err(AppError::Other("no claude binary name provided".into()));
    }
    // Fail fast if the binary isn't on the login-shell PATH, before we tear
    // down the running session.
    claude::resolve_named(&binary)?;

    let (claude_sid, transcript_path, repo_key) = store
        .read(|s| {
            s.find_workspace(&args.workspace_id).and_then(|w| {
                w.sessions
                    .iter()
                    .find(|sess| sess.id == args.session_meta_id)
                    .map(|sess| {
                        (
                            sess.claude_session_id.clone(),
                            sess.transcript_path.clone(),
                            sess.repo_key.clone(),
                        )
                    })
            })
        })
        .await
        .ok_or_else(|| {
            AppError::Other(format!(
                "no session {} in workspace {}",
                args.session_meta_id, args.workspace_id
            ))
        })?;

    // Resume only when the conversation is actually on disk; otherwise the new
    // binary starts a fresh session (an empty chat has nothing to resume).
    let resume_sid = claude_sid
        .as_deref()
        .filter(|_| transcript_is_resumable(transcript_path.as_deref()));

    // Kill the live tmux session so `spawn_claude` relaunches under the new
    // binary instead of reattaching the old process. Harmless if it already
    // exited.
    if !tmux_bin.0.as_os_str().is_empty() {
        tmux::kill_session(&tmux_bin.0, &args.session_meta_id);
    }

    start_session(StartSession {
        supervisor: &supervisor,
        store: &store,
        workspace_id: &args.workspace_id,
        repo_key,
        claude_bin: &claude_bin.0,
        tmux_bin: &tmux_bin.0,
        mcp: mcp.inner().as_ref(),
        resume_claude_sid: resume_sid,
        session_binary: Some(&binary),
        brief: None,
    })
    .await
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
    store: State<'_, Arc<Store>>,
    args: SetClaudeHiddenArgs,
) -> AppResult<()> {
    store
        .update_workspace(&args.workspace_id, |ws| {
            let meta = ws.session_mut(&args.session_id).ok_or_else(|| {
                AppError::Other(format!(
                    "session {} not found in workspace {}",
                    args.session_id, args.workspace_id
                ))
            })?;
            meta.hidden = args.hidden;
            Ok(())
        })
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SetWorkspaceNotesArgs {
    pub workspace_id: WorkspaceId,
    pub notes: String,
}

/// Persist the freeform notes for a workspace. The frontend holds the
/// authoritative text while editing and debounces calls here, so this does not
/// emit `workspace:changed` (doing so would churn the pane on every keystroke).
#[tauri::command]
pub async fn set_workspace_notes(
    store: State<'_, Arc<Store>>,
    args: SetWorkspaceNotesArgs,
) -> AppResult<()> {
    store
        .update_workspace_quiet(&args.workspace_id, |ws| {
            ws.notes = args.notes;
            Ok(())
        })
        .await
}

/// Newtype so `claude_bin` can be managed in Tauri state.
pub struct ClaudeBin(pub std::path::PathBuf);

#[tauri::command]
pub fn list_scripts(
    supervisor: State<'_, Arc<ScriptSupervisor>>,
    workspace_id: WorkspaceId,
) -> Vec<ScriptInfo> {
    supervisor.list_for_workspace(&workspace_id)
}

#[derive(Debug, Deserialize)]
pub struct StartScriptArgs {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub script_name: String,
}

/// Start a configured script in the given workspace+repo's worktree. Looks
/// up the command in the live registry, spawns it under tmux, and persists
/// a `ScriptRunMeta` so it can be reattached after a Tethys restart.
#[tauri::command]
pub async fn start_script(
    app: AppHandle,
    supervisor: State<'_, Arc<ScriptSupervisor>>,
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    tmux_bin: State<'_, TmuxBin>,
    args: StartScriptArgs,
) -> AppResult<ScriptInfo> {
    if tmux_bin.0.as_os_str().is_empty() {
        return Err(AppError::Other(
            "tmux not found — install with `brew install tmux` and restart Tethys".into(),
        ));
    }

    let reg = registry.require()?;
    let repo = reg.find_repo(&args.repo_key).ok_or_else(|| {
        AppError::Other(format!("unknown repo key: {}", args.repo_key))
    })?;
    let command = repo
        .scripts
        .get(&args.script_name)
        .cloned()
        .ok_or_else(|| {
            AppError::Other(format!(
                "no script '{}' configured for repo '{}'",
                args.script_name, args.repo_key
            ))
        })?;

    let cwd = store
        .read(|s| {
            s.find_workspace(&args.workspace_id)
                .and_then(|w| w.link(&args.repo_key))
                .map(|r| r.worktree_path.clone())
        })
        .await
        .ok_or_else(|| {
            AppError::Other(format!(
                "no worktree for {}/{} in state",
                args.workspace_id, args.repo_key
            ))
        })?;

    // Replace any prior run for the same (repo, script_name) so the UI only
    // ever shows one chip per configured script.
    let existing_ids: Vec<String> = supervisor
        .list_for_workspace(&args.workspace_id)
        .into_iter()
        .filter(|s| s.repo_key == args.repo_key && s.script_name == args.script_name)
        .map(|s| s.id)
        .collect();
    for sid in existing_ids {
        supervisor.dismiss(&sid, &tmux_bin.0);
    }
    store
        .mutate(|s| {
            if let Some(ws) = s.find_workspace_mut(&args.workspace_id) {
                ws.script_runs.retain(|m| {
                    !(m.repo_key == args.repo_key && m.script_name == args.script_name)
                });
            }
            Ok(())
        })
        .await?;

    let info = supervisor.start(
        args.workspace_id.clone(),
        args.repo_key.clone(),
        args.script_name.clone(),
        command.clone(),
        &cwd,
        &tmux_bin.0,
    )?;

    let meta = ScriptRunMeta {
        id: info.id.clone(),
        repo_key: args.repo_key,
        script_name: args.script_name,
        command,
        cwd,
        started_at: info.started_at,
    };
    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&args.workspace_id)
                .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;
            ws.script_runs.push(meta);
            Ok(())
        })
        .await?;

    let _ = app.emit(
        "script:changed",
        serde_json::json!({ "workspace_id": args.workspace_id }),
    );
    Ok(info)
}

#[derive(Debug, Deserialize)]
pub struct DismissScriptArgs {
    pub workspace_id: WorkspaceId,
    pub script_id: String,
}

/// Drop a script's in-memory handle (and kill any underlying tmux session
/// just in case). After this the chip disappears from the bar.
#[tauri::command]
pub async fn dismiss_script(
    app: AppHandle,
    supervisor: State<'_, Arc<ScriptSupervisor>>,
    store: State<'_, Arc<Store>>,
    tmux_bin: State<'_, TmuxBin>,
    args: DismissScriptArgs,
) -> AppResult<()> {
    if !tmux_bin.0.as_os_str().is_empty() {
        supervisor.dismiss(&args.script_id, &tmux_bin.0);
    }
    // The watcher would also do this on exit, but for an already-exited
    // script the watcher already ran — clean state here to be safe.
    store
        .mutate(|s| {
            if let Some(ws) = s.find_workspace_mut(&args.workspace_id) {
                ws.script_runs.retain(|m| m.id != args.script_id);
            }
            Ok(())
        })
        .await?;
    let _ = app.emit(
        "script:changed",
        serde_json::json!({ "workspace_id": args.workspace_id }),
    );
    Ok(())
}

#[tauri::command]
pub fn attach_script(
    supervisor: State<'_, Arc<ScriptSupervisor>>,
    script_id: String,
    on_bytes: tauri::ipc::Channel<InvokeResponseBody>,
) -> AppResult<Vec<u8>> {
    supervisor.attach(&script_id, on_bytes)
}

#[tauri::command]
pub fn detach_script(
    supervisor: State<'_, Arc<ScriptSupervisor>>,
    script_id: String,
    channel_id: u32,
) {
    supervisor.detach(&script_id, channel_id);
}

#[tauri::command]
pub fn send_input_script(
    supervisor: State<'_, Arc<ScriptSupervisor>>,
    script_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    supervisor.send_input(&script_id, &data)
}

#[tauri::command]
pub fn resize_script(
    supervisor: State<'_, Arc<ScriptSupervisor>>,
    script_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    supervisor.resize(&script_id, cols, rows)
}

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
pub fn detach_session(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    session_id: String,
    channel_id: u32,
) {
    supervisor.detach(&session_id, channel_id);
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

/// Seed `<workspace_root>/.claude/settings.local.json` from the workspace's
/// current set of repo links. Called once at workspace create; the file is
/// not regenerated thereafter. Best-effort: failures are surfaced as a
/// status event but never fail the parent command.
/// Extend an existing workspace-root settings.local.json with the entries
/// of a newly-added repo. Best-effort.
async fn append_repo_to_workspace_root_settings(
    workspace: &Workspace,
    repo_key: &str,
    paths: &Paths,
    tx: &JobTx,
) {
    let Some(workspace_root) = workspace.root_buf() else {
        return;
    };
    if let Err(e) = claude_local::append_repo_to_workspace_root_settings(
        &workspace_root,
        repo_key,
        paths,
    )
    .await
    {
        warn!(
            workspace = %workspace.id,
            repo = %repo_key,
            error = %e,
            "failed to extend workspace-root settings.local.json"
        );
        tx.status(
            format!("workspace-root settings extend failed: {e}"),
            None,
        );
    }
}

/// Rewrite `<workspace_root>/CLAUDE.md` from the workspace's repo links plus
/// the registry, so sessions know which repos are here, which aren't, and what
/// each one needs. Best-effort: a failure is a status event, never a failed
/// command.
async fn regen_workspace_claude_md(
    workspace: &Workspace,
    registry: &RepoRegistry,
    paths: &Paths,
    tx: &JobTx,
) {
    match workspace_doc::regenerate(workspace, registry, paths).await {
        Ok(Some(path)) => tx.status(format!("wrote {}", path.display()), None),
        Ok(None) => {}
        Err(e) => {
            warn!(
                workspace = %workspace.id,
                error = %e,
                "failed to write workspace-root CLAUDE.md"
            );
            tx.status(format!("workspace CLAUDE.md write failed: {e}"), None);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn transcript_none_is_not_resumable() {
        assert!(!transcript_is_resumable(None));
    }

    #[test]
    fn missing_transcript_is_not_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.jsonl");
        assert!(!transcript_is_resumable(Some(&path)));
    }

    #[test]
    fn empty_transcript_is_not_resumable() {
        // A fresh chat: claude reports a session id at startup but hasn't
        // written any conversation yet — `--resume` would fail.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::File::create(&path).unwrap();
        assert!(!transcript_is_resumable(Some(&path)));
    }

    #[test]
    fn nonempty_transcript_is_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("convo.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"user","message":"hi"}}"#).unwrap();
        assert!(transcript_is_resumable(Some(&path)));
    }
}
