use std::sync::Arc;

use chrono::Utc;
use serde::Deserialize;
use tauri::{ipc::Channel, AppHandle, Emitter, State};
use tracing::{info, warn};

use std::path::{Path, PathBuf};

use tauri::ipc::InvokeResponseBody;

use crate::agent::Agent;
use crate::artifacts::{Artifact, ArtifactKind, ArtifactStore};
use crate::branch_name;
use crate::agent_bin::{self, AgentBins};
use crate::claude_local;
use crate::error::{AppError, AppResult};
use crate::github::poller::{AuthSnapshot, GithubPoller};
use crate::github::{self, GithubPrStatus};
use crate::inprogress::InProgressWorkspaces;
use crate::job::{JobEvent, JobTx};
use crate::mcp::McpLaunch;
use crate::paths::Paths;
use crate::provision::{
    provision_repo_worktree, provision_workspace, teardown_repo_worktree, RepoProvision,
    RepoTeardown, WorkspaceProvision,
};
use crate::provision_queue::ProvisionQueue;
use crate::purge::Purger;
use crate::reconcile::{self, Discrepancies};
use crate::registry::{self, starter_template, RegistryLoad, Repo, RepoRegistry};
use crate::sessions::{open_session, OpenSession, SessionInfo, SessionSupervisor};
use crate::state::{Folder, FolderId, Origin, SystemErrorEntry, Workspace, WorkspaceId};
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
    /// `None` infers it from the reference, or the only GitHub-linked repo.
    #[serde(default)]
    pub repo_key: Option<String>,
    /// `123`, `#123`, `owner/repo#123`, or a full GitHub PR URL.
    pub reference: String,
}

#[tauri::command]
pub async fn attach_pr(
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    args: AttachPrArgs,
) -> AppResult<GithubPrStatus> {
    let attached = github::attach(
        &store,
        &registry,
        &args.workspace_id,
        args.repo_key.as_deref(),
        &args.reference,
    )
    .await?;
    Ok(attached.status)
}

#[derive(Debug, Deserialize)]
pub struct DetachPrArgs {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
    pub pr_number: u32,
}

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
            link.untrack(args.pr_number);
            Ok(())
        })
        .await?;
    Ok(())
}

const EDITOR_APP: &str = "Visual Studio Code";

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

/// Launched from Finder, Tethys's `PATH` lacks the `code` symlink.
const VSCODE_CLI_BUNDLED: &str =
    "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";

fn vscode_cli() -> &'static str {
    if Path::new(VSCODE_CLI_BUNDLED).exists() {
        VSCODE_CLI_BUNDLED
    } else {
        "code"
    }
}

/// `--reuse-window`, not `open -a`: every window over a full checkout is
/// another extension-host / TS-server stack with nothing shared.
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
pub fn clone_dir_path(paths: State<'_, Paths>) -> PathBuf {
    paths.repos_clone_dir()
}

/// Named rather than a path, so nothing else is openable this way.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigLocation {
    /// Seeded from the starter template if absent.
    ReposConfig,
    WorktreeRoot,
    CloneDir,
}

#[tauri::command]
pub fn open_config_location(
    location: ConfigLocation,
    paths: State<'_, Paths>,
    registry: State<'_, Arc<RegistryLoad>>,
) -> AppResult<()> {
    match location {
        ConfigLocation::ReposConfig => {
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
        ConfigLocation::WorktreeRoot => {
            open_workspace_in_editor(&registry.require()?.worktree_root)
        }
        ConfigLocation::CloneDir => {
            // Absent on a fresh install, and opening a missing path errors.
            let path = paths.repos_clone_dir();
            std::fs::create_dir_all(&path)?;
            open_workspace_in_editor(&path)
        }
    }
}

#[tauri::command]
pub async fn open_in_vscode(
    store: State<'_, Arc<Store>>,
    id: WorkspaceId,
) -> AppResult<()> {
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

/// Logical pixels in the main window.
#[tauri::command]
pub fn show_pr_view(
    app: AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport_height: f64,
) -> AppResult<()> {
    crate::pr_view::show(&app, url, x, y, width, height, viewport_height)
}

#[tauri::command]
pub fn hide_pr_view(app: AppHandle) -> AppResult<()> {
    crate::pr_view::hide(&app)
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceArgs {
    /// Frontend-minted, so the draft's row appears in place immediately.
    pub workspace_id: WorkspaceId,
    pub branch: String,
    pub repo_selections: Vec<String>,
    #[serde(default)]
    pub agent: Option<Agent>,
    #[serde(default)]
    pub agent_binary: Option<String>,
    #[serde(default)]
    pub folder: Option<FolderId>,
}

#[tauri::command]
pub async fn create_workspace(
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    paths: State<'_, Paths>,
    in_progress: State<'_, InProgressWorkspaces>,
    queue: State<'_, ProvisionQueue>,
    args: CreateWorkspaceArgs,
    on_event: Channel<JobEvent>,
) -> AppResult<Workspace> {
    let id = args.workspace_id.trim().to_string();
    if id.is_empty() {
        return Err(AppError::Other("workspace_id is required".into()));
    }
    let requested = args.branch.trim();
    if requested.is_empty() {
        return Err(AppError::Other("branch is required".into()));
    }
    if args.repo_selections.is_empty() {
        return Err(AppError::Other(
            "pick at least one repo to include in the workspace".into(),
        ));
    }
    let agent_binary = args
        .agent_binary
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(bin) = agent_binary.as_deref() {
        agent_bin::resolve_named(bin)?;
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

    let branch_name::Reserved {
        branch,
        workspace_dir,
        claim,
    } = branch_name::reserve(&reg.worktree_root, &in_progress, requested)?;

    let draft = Workspace::draft(
        id.clone(),
        branch.clone(),
        args.agent.unwrap_or_default(),
        agent_binary,
        Origin::Ui,
        args.folder.clone(),
    );
    store
        .mutate(|s| {
            // It may have been deleted while the dialog was open.
            if let Some(folder) = &draft.folder {
                if !s.folder_exists(folder) {
                    return Err(AppError::FolderNotFound(folder.clone()));
                }
            }
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
    if branch != requested {
        tx.status(format!("`{requested}` is taken; using `{branch}`"), None);
    }
    provision_workspace(WorkspaceProvision {
        workspace_id: &id,
        branch: &branch,
        workspace_dir: &workspace_dir,
        repos: &selected,
        registry: reg,
        paths: &paths,
        store: &store,
        claim,
        queue: &queue,
        tx: &tx,
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct AddRepoArgs {
    pub workspace_id: WorkspaceId,
    pub repo_key: String,
}

/// On failure, tears down only the new worktree.
#[tauri::command]
pub async fn add_repo_to_workspace(
    store: State<'_, Arc<Store>>,
    registry: State<'_, Arc<RegistryLoad>>,
    paths: State<'_, Paths>,
    queue: State<'_, ProvisionQueue>,
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

    // Runs the setup script, the expensive part of a create.
    let _slot = queue.acquire_announcing(&tx, Some(&args.repo_key)).await;

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
                    // Re-checked: provisioning took minutes. A link on a deleted
                    // workspace hands the purger a worktree it doesn't know of.
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
            // Backstop: `provision_repo_worktree` already cleans up, including
            // any branch it created.
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

/// Soft delete; the purger tears down once the grace window passes.
#[tauri::command]
pub async fn delete_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    tmux_bin: State<'_, TmuxBin>,
    id: WorkspaceId,
) -> AppResult<()> {
    let session_id: Option<String> = store
        .with_workspace(&id, |w| w.session.as_ref().map(|m| m.id.clone()))
        .await?;

    // Stop the agent writing to a worktree about to be removed.
    if let Some(sid) = session_id.filter(|_| !tmux_bin.0.as_os_str().is_empty()) {
        tmux::kill_session(&tmux_bin.0, &sid);
    }

    store
        .update_workspace(&id, |ws| {
            ws.deleted_at = Some(Utc::now());
            Ok(())
        })
        .await?;

    info!(%id, "soft-deleted workspace");
    let _ = app.emit("system_status:changed", &());
    Ok(())
}

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

/// Named ids move to the front in the given order; the rest keep their
/// relative order behind them.
#[tauri::command]
pub async fn reorder_workspaces(
    store: State<'_, Arc<Store>>,
    ids: Vec<WorkspaceId>,
) -> AppResult<()> {
    store
        .mutate(|s| {
            for id in &ids {
                if !s.workspaces.iter().any(|w| &w.id == id) {
                    return Err(AppError::WorkspaceNotFound(id.clone()));
                }
            }
            let mut moved: Vec<Workspace> = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(pos) = s.workspaces.iter().position(|w| &w.id == id) {
                    moved.push(s.workspaces.remove(pos));
                }
            }
            for ws in moved.into_iter().rev() {
                s.workspaces.insert(0, ws);
            }
            Ok(())
        })
        .await?;
    // No event: the frontend already reordered, and a refetch would flicker.
    Ok(())
}

#[tauri::command]
pub async fn list_folders(store: State<'_, Arc<Store>>) -> AppResult<Vec<Folder>> {
    Ok(store.read(|s| s.folders.clone()).await)
}

#[tauri::command]
pub async fn create_folder(store: State<'_, Arc<Store>>, name: String) -> AppResult<Folder> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Other("a folder needs a name".into()));
    }
    let folder = Folder::new(name);
    let created = folder.clone();
    store
        .mutate(move |s| {
            s.folders.push(folder);
            Ok(())
        })
        .await?;
    Ok(created)
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RenameFolderArgs {
    pub folder_id: FolderId,
    pub name: String,
}

#[tauri::command]
pub async fn rename_folder(
    store: State<'_, Arc<Store>>,
    args: RenameFolderArgs,
) -> AppResult<()> {
    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Other("a folder needs a name".into()));
    }
    store
        .mutate(|s| {
            let folder = s
                .find_folder_mut(&args.folder_id)
                .ok_or_else(|| AppError::FolderNotFound(args.folder_id.clone()))?;
            folder.name = name;
            Ok(())
        })
        .await
}

/// Its workspaces fall back to Default.
#[tauri::command]
pub async fn delete_folder(store: State<'_, Arc<Store>>, id: FolderId) -> AppResult<()> {
    store
        .mutate(|s| {
            if !s.folder_exists(&id) {
                return Err(AppError::FolderNotFound(id.clone()));
            }
            s.empty_folder(&id);
            s.folders.retain(|f| f.id != id);
            Ok(())
        })
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SetFolderCollapsedArgs {
    pub folder_id: FolderId,
    pub collapsed: bool,
}

#[tauri::command]
pub async fn set_folder_collapsed(
    store: State<'_, Arc<Store>>,
    args: SetFolderCollapsedArgs,
) -> AppResult<()> {
    store
        .mutate(|s| {
            let folder = s
                .find_folder_mut(&args.folder_id)
                .ok_or_else(|| AppError::FolderNotFound(args.folder_id.clone()))?;
            folder.collapsed = args.collapsed;
            Ok(())
        })
        .await
}

/// Same contract and silence as [`reorder_workspaces`].
#[tauri::command]
pub async fn reorder_folders(
    store: State<'_, Arc<Store>>,
    ids: Vec<FolderId>,
) -> AppResult<()> {
    store
        .mutate(|s| {
            for id in &ids {
                if !s.folder_exists(id) {
                    return Err(AppError::FolderNotFound(id.clone()));
                }
            }
            let mut moved: Vec<Folder> = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(pos) = s.folders.iter().position(|f| &f.id == id) {
                    moved.push(s.folders.remove(pos));
                }
            }
            for folder in moved.into_iter().rev() {
                s.folders.insert(0, folder);
            }
            Ok(())
        })
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MoveWorkspacesToFolderArgs {
    /// A whole blocker stack at once, so a move can't split one.
    pub workspace_ids: Vec<WorkspaceId>,
    pub folder: Option<FolderId>,
}

/// Silent, like [`reorder_workspaces`].
#[tauri::command]
pub async fn move_workspaces_to_folder(
    store: State<'_, Arc<Store>>,
    args: MoveWorkspacesToFolderArgs,
) -> AppResult<()> {
    let MoveWorkspacesToFolderArgs {
        workspace_ids,
        folder,
    } = args;
    store
        .mutate(|s| {
            if let Some(folder) = &folder {
                if !s.folder_exists(folder) {
                    return Err(AppError::FolderNotFound(folder.clone()));
                }
            }
            for id in &workspace_ids {
                if !s.workspaces.iter().any(|w| &w.id == id) {
                    return Err(AppError::WorkspaceNotFound(id.clone()));
                }
            }
            for id in &workspace_ids {
                if let Some(ws) = s.find_workspace_mut(id) {
                    ws.folder = folder.clone();
                }
            }
            Ok(())
        })
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SetWorkspaceBlockerArgs {
    pub workspace_id: WorkspaceId,
    pub blocker_id: Option<WorkspaceId>,
}

#[tauri::command]
pub async fn set_workspace_blocker(
    store: State<'_, Arc<Store>>,
    args: SetWorkspaceBlockerArgs,
) -> AppResult<()> {
    let SetWorkspaceBlockerArgs {
        workspace_id,
        blocker_id,
    } = args;
    let target = workspace_id.clone();
    store
        .mutate(|s| {
            if !s.workspaces.iter().any(|w| w.id == workspace_id) {
                return Err(AppError::WorkspaceNotFound(workspace_id.clone()));
            }
            if let Some(blocker) = &blocker_id {
                if !s.workspaces.iter().any(|w| &w.id == blocker) {
                    return Err(AppError::WorkspaceNotFound(blocker.clone()));
                }
                if s.blocker_would_cycle(&workspace_id, blocker) {
                    return Err(AppError::BlockerWouldCycle);
                }
                // Nesting is per folder: a cross-folder link would never draw.
                if s.folders_differ(&workspace_id, blocker) {
                    return Err(AppError::BlockerInAnotherFolder);
                }
            }
            if let Some(ws) = s.find_workspace_mut(&workspace_id) {
                ws.blocked_by = blocker_id;
            }
            Ok(())
        })
        .await?;
    store.notify_changed(&target);
    Ok(())
}

/// Still respects the grace window.
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

/// State-only removal: no git ops.
#[tauri::command]
pub async fn forget_workspace(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    tmux_bin: State<'_, TmuxBin>,
    id: WorkspaceId,
) -> AppResult<()> {
    let session_id: Option<String> = store
        .read(|s| {
            s.find_workspace(&id)
                .and_then(|w| w.session.as_ref().map(|m| m.id.clone()))
        })
        .await;

    let removed = store
        .mutate(|s| {
            let before = s.workspaces.len();
            s.workspaces.retain(|w| w.id != id);
            s.clear_links_to(&id);
            Ok(s.workspaces.len() < before)
        })
        .await?;
    if !removed {
        return Err(AppError::WorkspaceNotFound(id));
    }

    if let Some(sid) = session_id.filter(|_| !tmux_bin.0.as_os_str().is_empty()) {
        tmux::kill_session(&tmux_bin.0, &sid);
    }

    info!(%id, "forgot workspace (state-only removal)");
    emit_workspace_changed(&app, &id);
    Ok(())
}

/// `None` until this run has spawned or reattached the session.
#[tauri::command]
pub async fn get_session(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    workspace_id: WorkspaceId,
) -> AppResult<Option<SessionInfo>> {
    let session_id = store
        .read(|s| {
            s.find_workspace(&workspace_id)
                .and_then(|w| w.session.as_ref().map(|m| m.id.clone()))
        })
        .await;
    Ok(session_id.and_then(|id| supervisor.info(&id)))
}

#[tauri::command]
pub async fn acknowledge_session_turn(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    workspace_id: WorkspaceId,
) -> AppResult<()> {
    let session_id = store
        .with_workspace(&workspace_id, |w| w.session.as_ref().map(|m| m.id.clone()))
        .await?;
    if let Some(session_id) = session_id {
        supervisor.acknowledge_turn(&session_id, &workspace_id).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn start_agent_session(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    agent_bins: State<'_, AgentBins>,
    tmux_bin: State<'_, TmuxBin>,
    paths: State<'_, Paths>,
    mcp: State<'_, Option<McpLaunch>>,
    workspace_id: WorkspaceId,
) -> AppResult<SessionInfo> {
    open_session(OpenSession {
        supervisor: &supervisor,
        store: &store,
        workspace_id: &workspace_id,
        agent_bins: &agent_bins,
        tmux_bin: &tmux_bin.0,
        paths: &paths,
        mcp: mcp.inner().as_ref(),
        brief: None,
    })
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct SwitchAgentArgs {
    pub workspace_id: WorkspaceId,
    /// Sent rather than inferred from the binary's name, which can be anything.
    pub agent: Agent,
    pub agent_binary: String,
}

#[tauri::command]
pub async fn switch_agent(
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    agent_bins: State<'_, AgentBins>,
    tmux_bin: State<'_, TmuxBin>,
    paths: State<'_, Paths>,
    mcp: State<'_, Option<McpLaunch>>,
    args: SwitchAgentArgs,
) -> AppResult<SessionInfo> {
    let binary = args.agent_binary.trim().to_string();
    if binary.is_empty() {
        return Err(AppError::Other("no agent binary name provided".into()));
    }
    // Before tearing down the running session.
    agent_bin::resolve_named(&binary)?;

    let session_id = store
        .update_workspace(&args.workspace_id, |ws| {
            let previous = ws.session.as_ref().map(|m| m.id.clone());
            // Neither CLI can read the other's transcript.
            if ws.agent != args.agent {
                ws.session = None;
            }
            ws.agent = args.agent;
            ws.agent_binary = Some(binary.clone());
            Ok(previous)
        })
        .await?;

    // So `open_session` can't just reattach the old process.
    if let Some(sid) = session_id.filter(|_| !tmux_bin.0.as_os_str().is_empty()) {
        tmux::kill_session(&tmux_bin.0, &sid);
    }

    open_session(OpenSession {
        supervisor: &supervisor,
        store: &store,
        workspace_id: &args.workspace_id,
        agent_bins: &agent_bins,
        tmux_bin: &tmux_bin.0,
        paths: &paths,
        mcp: mcp.inner().as_ref(),
        brief: None,
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct SetWorkspaceNotesArgs {
    pub workspace_id: WorkspaceId,
    pub notes: String,
}

/// Quiet: the frontend owns the text while editing, and an event per debounced
/// keystroke would churn the pane.
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

#[tauri::command]
pub async fn list_artifacts(
    artifacts: State<'_, Arc<ArtifactStore>>,
    workspace_id: WorkspaceId,
) -> AppResult<Vec<Artifact>> {
    Ok(artifacts.list(&workspace_id).await)
}

/// Not plugin-opener's `openPath`, which is scoped to paths fixed in the
/// capability file. Takes an artifact id so no arbitrary path can be opened.
#[tauri::command]
pub async fn open_artifact(
    artifacts: State<'_, Arc<ArtifactStore>>,
    workspace_id: WorkspaceId,
    artifact_id: String,
) -> AppResult<()> {
    let path = artifacts
        .list(&workspace_id)
        .await
        .into_iter()
        .find(|a| a.id == artifact_id)
        .and_then(|a| match a.kind {
            ArtifactKind::Page { path } => Some(path),
            ArtifactKind::Diagram { .. } => None,
        })
        .ok_or_else(|| AppError::Other("no such page".into()))?;
    std::process::Command::new("open")
        .arg(&path)
        .status()
        .map_err(|e| AppError::Other(format!("failed to open {}: {e}", path.display())))?;
    Ok(())
}

#[tauri::command]
pub async fn dismiss_artifact(
    artifacts: State<'_, Arc<ArtifactStore>>,
    workspace_id: WorkspaceId,
    artifact_id: String,
) -> AppResult<()> {
    artifacts.dismiss(&workspace_id, &artifact_id).await;
    Ok(())
}

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
    supervisor.send_input(&session_id, &data)
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

/// WKWebView hides a pasted file's path, and its own auto-insert always lands
/// in Claude Code as `[Image #N]`, whatever the file is.
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
