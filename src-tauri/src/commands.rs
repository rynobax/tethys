use std::ops::Deref;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use tracing::{error, info};

use crate::agent::Agent;
use crate::artifacts::{Artifact, ArtifactKind};
use crate::branch_name;
use crate::budget::OverrideRequest;
use crate::agent_bin;
use crate::error::{AppError, AppResult};
use crate::github::poller::AuthSnapshot;
use crate::github::{self, GithubPrStatus};
use crate::job::{JobEvent, JobTx};
use crate::provision::{add_repos, provision_workspace, RepoAddition, WorkspaceProvision};
use crate::reconcile::{self, Discrepancies};
use crate::registry::{starter_template, RegistryLoad};
use crate::sessions::{open_session, OpenSession, SessionInfo};
use crate::shipping;
use crate::state::{Folder, FolderId, Origin, PrRef, SystemErrorEntry, Workspace, WorkspaceId};
use crate::theme::{self, Theme};
use crate::tmux;
use crate::ui_socket::{Conn, JsonChannel};
use crate::Daemon;

/// What a command sees: the daemon, and the connection that called it.
pub struct Ctx<'a> {
    pub d: &'a Daemon,
    pub conn: &'a Conn,
}

impl Deref for Ctx<'_> {
    type Target = Daemon;

    fn deref(&self) -> &Daemon {
        self.d
    }
}

/// Generates `dispatch`. Each entry is a handler below, called with its
/// parameters read from the request's `args` by Tauri's convention, which the
/// frontend's wrappers were written against: top-level keys are the
/// parameter names in camelCase, while keys inside an `args` struct are
/// snake_case as declared.
macro_rules! ui_commands {
    ($($name:ident($($param:ident: $ty:ty),* $(,)?)),* $(,)?) => {
        pub async fn dispatch(cx: &Ctx<'_>, cmd: &str, args: Value) -> AppResult<Value> {
            match cmd {
                $(stringify!($name) => {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Params {
                        $($param: $ty),*
                    }
                    let Params { $($param),* } = serde_json::from_value(args).map_err(|e| {
                        AppError::Other(format!("invalid args for command `{cmd}`: {e}"))
                    })?;
                    Ok(serde_json::to_value($name(cx, $($param),*).await?)?)
                })*
                _ => Err(AppError::Other(format!("unknown command `{cmd}`"))),
            }
        }
    };
}

// Read by `scripts/check-ipc-parity.mjs`: one command per line.
ui_commands! {
    list_workspaces(),
    list_budget_requests(),
    decide_budget_request(workspace_id: WorkspaceId, approve: bool),
    create_workspace(args: CreateWorkspaceArgs, on_event: u32),
    add_repo_to_workspace(args: AddRepoArgs, on_event: u32),
    delete_workspace(id: WorkspaceId),
    cancel_delete_workspace(id: WorkspaceId),
    list_folders(),
    create_folder(name: String),
    rename_folder(args: RenameFolderArgs),
    delete_folder(id: FolderId),
    set_folder_collapsed(args: SetFolderCollapsedArgs),
    reorder_folders(ids: Vec<FolderId>),
    move_workspaces_to_folder(args: MoveWorkspacesToFolderArgs),
    reorder_workspaces(ids: Vec<WorkspaceId>),
    run_purge_now(),
    list_system_errors(),
    dismiss_system_error(id: String),
    registry_status(),
    github_auth_status(),
    github_reprobe_auth(),
    request_github_poll(),
    attach_pr(args: AttachPrArgs),
    detach_pr(args: DetachPrArgs),
    open_config_location(location: ConfigLocation),
    clone_dir_path(),
    open_in_vscode(id: WorkspaceId),
    list_discrepancies(),
    remove_orphan_dir(path: PathBuf),
    forget_workspace(id: WorkspaceId),
    get_session(workspace_id: WorkspaceId),
    acknowledge_session_turn(workspace_id: WorkspaceId),
    start_agent_session(workspace_id: WorkspaceId),
    switch_agent(args: SwitchAgentArgs),
    set_workspace_notes(args: SetWorkspaceNotesArgs),
    start_shipping(args: StartShippingArgs),
    stop_shipping(workspace_id: WorkspaceId),
    list_artifacts(workspace_id: WorkspaceId),
    dismiss_artifact(workspace_id: WorkspaceId, artifact_id: String),
    open_artifact(workspace_id: WorkspaceId, artifact_id: String),
    attach_session(session_id: String, on_bytes: u32),
    detach_session(session_id: String, channel_id: u32),
    send_input(session_id: String, data: Vec<u8>),
    resize_session(session_id: String, cols: u16, rows: u16),
    get_theme(),
    load_theme(path: PathBuf),
    reset_theme(),
    read_clipboard_file_paths(),
}

pub async fn list_workspaces(cx: &Ctx<'_>) -> AppResult<Vec<Workspace>> {
    Ok(cx.store.read(|s| s.workspaces.clone()).await)
}

pub async fn registry_status(cx: &Ctx<'_>) -> AppResult<RegistryLoad> {
    Ok((*cx.registry).clone())
}

pub async fn github_auth_status(cx: &Ctx<'_>) -> AppResult<AuthSnapshot> {
    Ok(cx.poller.auth_snapshot().await)
}

pub async fn github_reprobe_auth(cx: &Ctx<'_>) -> AppResult<AuthSnapshot> {
    cx.poller.probe_login().await;
    Ok(cx.poller.auth_snapshot().await)
}

/// The window took focus.
pub async fn request_github_poll(cx: &Ctx<'_>) -> AppResult<()> {
    cx.poller.request_tick().await;
    Ok(())
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

pub async fn attach_pr(cx: &Ctx<'_>, args: AttachPrArgs) -> AppResult<GithubPrStatus> {
    let attached = github::attach(
        &cx.store,
        &cx.registry,
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

pub async fn detach_pr(cx: &Ctx<'_>, args: DetachPrArgs) -> AppResult<()> {
    cx.store
        .update_workspace(&args.workspace_id, |ws| {
            let link = ws.link_mut(&args.repo_key).ok_or_else(|| {
                AppError::Other(format!("workspace has no worktree for {}", args.repo_key))
            })?;
            link.untrack(args.pr_number);
            crate::shipping::settle_workspace(ws);
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

pub async fn clone_dir_path(cx: &Ctx<'_>) -> AppResult<PathBuf> {
    Ok(cx.paths.repos_clone_dir())
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

pub async fn open_config_location(cx: &Ctx<'_>, location: ConfigLocation) -> AppResult<()> {
    match location {
        ConfigLocation::ReposConfig => {
            let path = cx.paths.repos_config_file();
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
            open_workspace_in_editor(&cx.registry.require()?.worktree_root)
        }
        ConfigLocation::CloneDir => {
            // Absent on a fresh install, and opening a missing path errors.
            let path = cx.paths.repos_clone_dir();
            std::fs::create_dir_all(&path)?;
            open_workspace_in_editor(&path)
        }
    }
}

pub async fn open_in_vscode(cx: &Ctx<'_>, id: WorkspaceId) -> AppResult<()> {
    let workspace_root: PathBuf = cx
        .store
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
    /// Frontend-minted, so the draft's row appears in place immediately.
    pub workspace_id: WorkspaceId,
    pub branch: String,
    pub agent: Agent,
    pub agent_binary: Option<String>,
    pub folder: Option<FolderId>,
}

pub async fn create_workspace(
    cx: &Ctx<'_>,
    args: CreateWorkspaceArgs,
    on_event: u32,
) -> AppResult<Workspace> {
    let id = args.workspace_id.trim().to_string();
    if id.is_empty() {
        return Err(AppError::Other("workspace_id is required".into()));
    }
    let requested = args.branch.trim();
    if requested.is_empty() {
        return Err(AppError::Other("branch is required".into()));
    }
    let agent_binary = args
        .agent_binary
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(bin) = agent_binary.as_deref() {
        agent_bin::resolve_named(bin)?;
    }

    let reg = cx.registry.require()?;
    let selected = reg.enabled_repos();
    if selected.is_empty() {
        return Err(AppError::Other(
            "every repo in repos.toml is disabled; enable one to create a workspace".into(),
        ));
    }

    let branch_name::Reserved {
        branch,
        workspace_dir,
        claim,
    } = branch_name::reserve(&reg.worktree_root, &cx.in_progress, requested)?;

    let draft = Workspace::draft(
        id.clone(),
        branch.clone(),
        args.agent,
        agent_binary,
        Origin::Ui,
        args.folder.clone(),
    );
    cx.store
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
    cx.store.notify_changed(&id);

    let tx = spawn_event_forwarder(cx.conn.json_channel(on_event));
    if branch != requested {
        tx.status(format!("`{requested}` is taken; using `{branch}`"), None);
    }
    provision_workspace(WorkspaceProvision {
        workspace_id: &id,
        branch: &branch,
        workspace_dir: &workspace_dir,
        repos: &selected,
        registry: reg,
        paths: &cx.paths,
        store: &cx.store,
        claim,
        queue: &cx.queue,
        spares: &cx.spares,
        tx: &tx,
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct AddRepoArgs {
    pub workspace_id: WorkspaceId,
    pub repo_keys: Vec<String>,
}

pub async fn add_repo_to_workspace(
    cx: &Ctx<'_>,
    args: AddRepoArgs,
    on_event: u32,
) -> AppResult<Workspace> {
    let reg = cx.registry.require()?;
    let tx = spawn_event_forwarder(cx.conn.json_channel(on_event));
    add_repos(RepoAddition {
        workspace_id: &args.workspace_id,
        repo_keys: &args.repo_keys,
        registry: reg,
        paths: &cx.paths,
        store: &cx.store,
        queue: &cx.queue,
        tx: &tx,
    })
    .await
}

/// Soft delete; the purger tears down once the grace window passes.
pub async fn delete_workspace(cx: &Ctx<'_>, id: WorkspaceId) -> AppResult<()> {
    let children = cx.store.read(|s| s.live_children(&id)).await;
    if children > 0 {
        return Err(AppError::HasChildren(children));
    }
    let session_id: Option<String> = cx
        .store
        .with_workspace(&id, |w| w.session.as_ref().map(|m| m.id.clone()))
        .await?;

    // Stop the agent writing to a worktree about to be removed.
    if let Some(sid) = session_id {
        tmux::kill_session(&cx.tmux_bin, &sid);
    }

    let deleted = cx
        .store
        .update_workspace(&id, |ws| {
            ws.deleted_at = Some(Utc::now());
            Ok(ws.clone())
        })
        .await?;

    info!(%id, "soft-deleted workspace");
    cx.events.emit("system_status:changed", ());

    // Fetching the child's PRs can take seconds; the delete shouldn't wait.
    if deleted.parent.is_some() {
        let store = cx.store.clone();
        let supervisor = cx.supervisor.clone();
        let registry = cx.registry.clone();
        tokio::spawn(async move {
            crate::child_events::child_deleted(&store, &supervisor, &registry, deleted).await;
        });
    }
    Ok(())
}

pub async fn cancel_delete_workspace(cx: &Ctx<'_>, id: WorkspaceId) -> AppResult<()> {
    cx.store
        .update_workspace(&id, |ws| {
            ws.deleted_at = None;
            Ok(())
        })
        .await?;
    cx.events.emit("system_status:changed", ());
    Ok(())
}

/// Named ids move to the front in the given order; the rest keep their
/// relative order behind them.
pub async fn reorder_workspaces(cx: &Ctx<'_>, ids: Vec<WorkspaceId>) -> AppResult<()> {
    cx.store
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

pub async fn list_folders(cx: &Ctx<'_>) -> AppResult<Vec<Folder>> {
    Ok(cx.store.read(|s| s.folders.clone()).await)
}

pub async fn create_folder(cx: &Ctx<'_>, name: String) -> AppResult<Folder> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Other("a folder needs a name".into()));
    }
    let folder = Folder::new(name);
    let created = folder.clone();
    cx.store
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

pub async fn rename_folder(cx: &Ctx<'_>, args: RenameFolderArgs) -> AppResult<()> {
    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Other("a folder needs a name".into()));
    }
    cx.store
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
pub async fn delete_folder(cx: &Ctx<'_>, id: FolderId) -> AppResult<()> {
    cx.store
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

pub async fn set_folder_collapsed(cx: &Ctx<'_>, args: SetFolderCollapsedArgs) -> AppResult<()> {
    cx.store
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
pub async fn reorder_folders(cx: &Ctx<'_>, ids: Vec<FolderId>) -> AppResult<()> {
    cx.store
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
    /// A parent with all its descendants, so a move can't split them.
    pub workspace_ids: Vec<WorkspaceId>,
    pub folder: Option<FolderId>,
}

/// Silent, like [`reorder_workspaces`].
pub async fn move_workspaces_to_folder(
    cx: &Ctx<'_>,
    args: MoveWorkspacesToFolderArgs,
) -> AppResult<()> {
    let MoveWorkspacesToFolderArgs {
        workspace_ids,
        folder,
    } = args;
    cx.store
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

/// Still respects the grace window.
pub async fn run_purge_now(cx: &Ctx<'_>) -> AppResult<()> {
    cx.purger.request_tick();
    Ok(())
}

pub async fn list_system_errors(cx: &Ctx<'_>) -> AppResult<Vec<SystemErrorEntry>> {
    Ok(cx.store.read(|s| s.system_errors.clone()).await)
}

pub async fn dismiss_system_error(cx: &Ctx<'_>, id: String) -> AppResult<()> {
    cx.store
        .mutate(|s| {
            s.system_errors.retain(|e| e.id != id);
            Ok(())
        })
        .await?;
    cx.events.emit("system_status:changed", ());
    Ok(())
}

pub async fn list_discrepancies(cx: &Ctx<'_>) -> AppResult<Discrepancies> {
    let snapshot = cx.store.read(|s| s.clone()).await;
    let pending = cx.in_progress.snapshot();
    let reg = match &*cx.registry {
        RegistryLoad::Ok { registry, .. } => Some(registry),
        _ => None,
    };
    Ok(reconcile::scan(&snapshot, reg, &pending).await)
}

pub async fn remove_orphan_dir(cx: &Ctx<'_>, path: PathBuf) -> AppResult<()> {
    let reg = cx.registry.require()?;
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
pub async fn forget_workspace(cx: &Ctx<'_>, id: WorkspaceId) -> AppResult<()> {
    let session_id: Option<String> = cx
        .store
        .read(|s| {
            s.find_workspace(&id)
                .and_then(|w| w.session.as_ref().map(|m| m.id.clone()))
        })
        .await;

    let removed = cx
        .store
        .mutate(|s| {
            let before = s.workspaces.len();
            s.workspaces.retain(|w| w.id != id);
            s.orphan_children_of(&id);
            Ok(s.workspaces.len() < before)
        })
        .await?;
    if !removed {
        return Err(AppError::WorkspaceNotFound(id));
    }

    if let Some(sid) = session_id {
        tmux::kill_session(&cx.tmux_bin, &sid);
    }

    info!(%id, "forgot workspace (state-only removal)");
    cx.store.notify_changed(&id);
    Ok(())
}

/// `None` until this run has spawned or reattached the session.
pub async fn get_session(cx: &Ctx<'_>, workspace_id: WorkspaceId) -> AppResult<Option<SessionInfo>> {
    let session_id = cx
        .store
        .read(|s| {
            s.find_workspace(&workspace_id)
                .and_then(|w| w.session.as_ref().map(|m| m.id.clone()))
        })
        .await;
    Ok(session_id.and_then(|id| cx.supervisor.info(&id)))
}

pub async fn list_budget_requests(cx: &Ctx<'_>) -> AppResult<Vec<OverrideRequest>> {
    Ok(cx.budgets.pending())
}

pub async fn decide_budget_request(
    cx: &Ctx<'_>,
    workspace_id: WorkspaceId,
    approve: bool,
) -> AppResult<()> {
    cx.budgets.decide(&workspace_id, approve)
}

pub async fn acknowledge_session_turn(cx: &Ctx<'_>, workspace_id: WorkspaceId) -> AppResult<()> {
    let session_id = cx
        .store
        .with_workspace(&workspace_id, |w| w.session.as_ref().map(|m| m.id.clone()))
        .await?;
    if let Some(session_id) = session_id {
        cx.supervisor.acknowledge_turn(&session_id, &workspace_id).await;
    }
    Ok(())
}

pub async fn start_agent_session(cx: &Ctx<'_>, workspace_id: WorkspaceId) -> AppResult<SessionInfo> {
    open_session(OpenSession {
        supervisor: &cx.supervisor,
        store: &cx.store,
        workspace_id: &workspace_id,
        agent_bins: &cx.agent_bins,
        tmux_bin: &cx.tmux_bin,
        paths: &cx.paths,
        mcp: cx.mcp.as_ref(),
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

pub async fn switch_agent(cx: &Ctx<'_>, args: SwitchAgentArgs) -> AppResult<SessionInfo> {
    let binary = args.agent_binary.trim().to_string();
    if binary.is_empty() {
        return Err(AppError::Other("no agent binary name provided".into()));
    }
    // Before tearing down the running session.
    agent_bin::resolve_named(&binary)?;

    let session_id = cx
        .store
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
    if let Some(sid) = session_id {
        tmux::kill_session(&cx.tmux_bin, &sid);
    }

    open_session(OpenSession {
        supervisor: &cx.supervisor,
        store: &cx.store,
        workspace_id: &args.workspace_id,
        agent_bins: &cx.agent_bins,
        tmux_bin: &cx.tmux_bin,
        paths: &cx.paths,
        mcp: cx.mcp.as_ref(),
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
pub async fn set_workspace_notes(cx: &Ctx<'_>, args: SetWorkspaceNotesArgs) -> AppResult<()> {
    cx.store
        .update_workspace_quiet(&args.workspace_id, |ws| {
            ws.notes = args.notes;
            Ok(())
        })
        .await
}

#[derive(Debug, Deserialize)]
pub struct StartShippingArgs {
    pub workspace_id: WorkspaceId,
    /// Empty for the workspace's only open PR.
    #[serde(default)]
    pub prs: Vec<PrRef>,
}

pub async fn start_shipping(cx: &Ctx<'_>, args: StartShippingArgs) -> AppResult<()> {
    let nudged = cx
        .store
        .update_workspace(&args.workspace_id, |ws| shipping::start(ws, args.prs))
        .await?;
    if nudged {
        let shipper = cx.shipper.clone();
        tokio::spawn(async move { shipper.wake(&args.workspace_id).await });
    }
    Ok(())
}

/// Also takes a workspace out of Shipped, back among the ones being worked on.
pub async fn stop_shipping(cx: &Ctx<'_>, workspace_id: WorkspaceId) -> AppResult<()> {
    cx.store
        .update_workspace(&workspace_id, |ws| {
            ws.shipment = None;
            Ok(())
        })
        .await?;
    info!(%workspace_id, "workspace taken out of shipping");
    Ok(())
}

pub async fn list_artifacts(cx: &Ctx<'_>, workspace_id: WorkspaceId) -> AppResult<Vec<Artifact>> {
    Ok(cx.artifacts.list(&workspace_id).await)
}

/// Takes an artifact id so no arbitrary path can be opened.
pub async fn open_artifact(
    cx: &Ctx<'_>,
    workspace_id: WorkspaceId,
    artifact_id: String,
) -> AppResult<()> {
    let path = cx
        .artifacts
        .list(&workspace_id)
        .await
        .into_iter()
        .find(|a| a.id == artifact_id)
        .and_then(|a| match a.kind {
            ArtifactKind::Page { path } => Some(path),
            ArtifactKind::Review { .. } => None,
        })
        .ok_or_else(|| AppError::Other("no such page".into()))?;
    std::process::Command::new("open")
        .arg(&path)
        .status()
        .map_err(|e| AppError::Other(format!("failed to open {}: {e}", path.display())))?;
    Ok(())
}

pub async fn dismiss_artifact(
    cx: &Ctx<'_>,
    workspace_id: WorkspaceId,
    artifact_id: String,
) -> AppResult<()> {
    cx.artifacts.dismiss(&workspace_id, &artifact_id).await;
    Ok(())
}

/// The scrollback arrives as `on_bytes`'s first frame, not in the reply: on
/// one ordered socket, a live chunk could otherwise beat the reply to the page.
pub async fn attach_session(cx: &Ctx<'_>, session_id: String, on_bytes: u32) -> AppResult<()> {
    cx.supervisor
        .attach(&session_id, cx.conn.byte_channel(on_bytes))
}

pub async fn detach_session(cx: &Ctx<'_>, session_id: String, channel_id: u32) -> AppResult<()> {
    cx.supervisor.detach(&session_id, cx.conn.id(), channel_id);
    Ok(())
}

pub async fn send_input(cx: &Ctx<'_>, session_id: String, data: Vec<u8>) -> AppResult<()> {
    cx.supervisor.send_input(&session_id, &data)
}

pub async fn resize_session(
    cx: &Ctx<'_>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    cx.supervisor.resize(&session_id, cols, rows)
}

pub async fn get_theme(cx: &Ctx<'_>) -> AppResult<Option<Theme>> {
    Theme::load_saved(&cx.paths.theme_file())
}

/// The shell owns the file dialog; this is what follows it.
pub async fn load_theme(cx: &Ctx<'_>, path: PathBuf) -> AppResult<Theme> {
    theme::load_and_emit(&cx.events, &path, &cx.paths.theme_file()).inspect_err(|e| {
        error!(error = %e, source = %path.display(), "load theme failed");
    })
}

pub async fn reset_theme(cx: &Ctx<'_>) -> AppResult<()> {
    theme::clear_and_emit(&cx.events, &cx.paths.theme_file())
}

/// The renderer can't see a pasted file's path, and Claude Code turns a pasted
/// image into `[Image #N]`, whatever the file is.
pub async fn read_clipboard_file_paths(_cx: &Ctx<'_>) -> AppResult<Vec<String>> {
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

fn spawn_event_forwarder(channel: JsonChannel) -> JobTx {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if !channel.send(&event) {
                break;
            }
        }
    });
    JobTx(tx)
}
