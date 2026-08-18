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
use crate::github::poller::{fetch_pr_status, AuthSnapshot, GithubPoller};
use crate::github::{parse_pr_reference, GithubPrStatus, GithubSlug};
use crate::inprogress::InProgressWorkspaces;
use crate::job::{JobEvent, JobTx};
use crate::paths::Paths;
use crate::purge::Purger;
use crate::reconcile::{self, Discrepancies};
use crate::registry::{self, starter_template, RegistryLoad, Repo, RepoRegistry};
use crate::scripts::{ScriptInfo, ScriptSupervisor};
use crate::sessions::{SessionInfo, SessionSupervisor};
use crate::setup;
use crate::state::{
    AttachedPr, ClaudeSessionMeta, RepoLink, ScriptRunMeta, SystemErrorEntry, Workspace,
    WorkspaceId, WorkspaceStatus,
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
    app: AppHandle,
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

    let (repo_key, slug) = match (&args.repo_key, &pr.slug) {
        (Some(key), _) => candidates
            .into_iter()
            .find(|(k, _)| k == key)
            .ok_or_else(|| {
                AppError::Other(format!("{key} isn't a GitHub-linked repo in this workspace"))
            })?,
        (None, Some(want)) => candidates
            .into_iter()
            .find(|(_, slug)| slug == want)
            .ok_or_else(|| {
                AppError::Other(format!(
                    "no repo in this workspace points at {}/{}",
                    want.owner, want.name
                ))
            })?,
        (None, None) if candidates.len() == 1 => candidates.remove(0),
        (None, None) => {
            return Err(AppError::Other(
                "this workspace has more than one GitHub repo — pick which one the PR belongs to"
                    .to_string(),
            ))
        }
    };
    // An explicit repo_key plus a URL for a different repo is a mistake worth
    // reporting rather than silently trusting one over the other.
    if let Some(want) = &pr.slug {
        if want != &slug {
            return Err(AppError::Other(format!(
                "PR #{} is in {}/{}, but repo {repo_key} points at {}/{}",
                pr.number, want.owner, want.name, slug.owner, slug.name
            )));
        }
    }

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
        .mutate(move |s| {
            let ws = s
                .find_workspace_mut(&workspace_id)
                .ok_or_else(|| AppError::WorkspaceNotFound(workspace_id.clone()))?;
            let link = ws
                .repo_links
                .iter_mut()
                .find(|r| r.repo_key == repo_key)
                .ok_or_else(|| {
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

    emit_workspace_changed(&app, &args.workspace_id);
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
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    args: DetachPrArgs,
) -> AppResult<()> {
    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&args.workspace_id)
                .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;
            let link = ws
                .repo_links
                .iter_mut()
                .find(|r| r.repo_key == args.repo_key)
                .ok_or_else(|| {
                    AppError::Other(format!("workspace has no worktree for {}", args.repo_key))
                })?;
            link.attached_prs.retain(|a| a.number != args.pr_number);
            Ok(())
        })
        .await?;

    emit_workspace_changed(&app, &args.workspace_id);
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

struct RepoProvision<'a> {
    repo: &'a Repo,
    worktree_path: &'a Path,
    branch: &'a str,
    paths: &'a Paths,
    tx: &'a JobTx,
}

/// Clone (if needed) → pull → resolve branch → worktree add → install
/// `.claude/settings.local.json` symlink → run setup script. Returns the
/// `RepoLink` to push into state. Atomic for its own repo: any failure past
/// `worktree add` tears down this repo's worktree (and its branch, if Tethys
/// created it) before bubbling. Sibling repos remain the caller's
/// responsibility (we don't know whether more still need provisioning).
async fn provision_repo_worktree(ctx: RepoProvision<'_>) -> AppResult<RepoLink> {
    let clone_path = ctx.paths.repo_clone_path(&ctx.repo.key);

    git::ensure_clone(&clone_path, &ctx.repo.remote_url, ctx.tx, &ctx.repo.key).await?;
    // A stray checkout in the clone would otherwise feed the wrong base into
    // the pull (fast-forwards HEAD) and into any `track_from = None` worktree
    // (branches off HEAD). Put it back on the default branch first.
    git::ensure_clone_on_default_branch(
        &clone_path,
        ctx.repo.default_branch.as_deref(),
        ctx.tx,
        &ctx.repo.key,
    )
    .await?;
    git::pull_clone(&clone_path, ctx.tx, &ctx.repo.key).await?;

    // Decide how the worktree's branch is resolved. An already-existing local
    // branch is checked out as-is (e.g. editing a PR branch locally); git
    // refuses if it's already checked out by another workspace. A branch that
    // only exists on the remote gets a fresh local tracking branch. Otherwise
    // we branch off HEAD.
    let branch_preexisted = git::branch_exists(&clone_path, ctx.branch).await?;
    let remote_start = if !branch_preexisted
        && git::remote_branch_exists(&clone_path, "origin", ctx.branch).await?
    {
        Some(format!("origin/{}", ctx.branch))
    } else {
        None
    };
    let source = match (branch_preexisted, &remote_start) {
        (true, _) => git::WorktreeBranch::ExistingLocal,
        (false, Some(start)) => git::WorktreeBranch::TrackRemote(start),
        (false, None) => git::WorktreeBranch::NewFromHead,
    };
    // Tethys "owns" (and may delete on teardown) only branches it creates.
    let created_branch = !branch_preexisted;

    if branch_preexisted {
        ctx.tx.status(
            format!("checking out existing branch {}", ctx.branch),
            Some(&ctx.repo.key),
        );
    }

    // Everything past `worktree_add` leaves on-disk state behind, so on failure
    // we tear down this repo's own worktree (and its branch, only if we created
    // it) before bubbling. Sibling repos are the caller's responsibility.
    let provisioned = async {
        git::worktree_add(
            &clone_path,
            ctx.worktree_path,
            ctx.branch,
            source,
            ctx.tx,
            &ctx.repo.key,
        )
        .await?;

        claude_local::install_symlink(ctx.worktree_path, ctx.paths, ctx.tx, &ctx.repo.key).await?;

        copy_configured_files(
            &clone_path,
            ctx.worktree_path,
            &ctx.repo.copy_files,
            ctx.tx,
            &ctx.repo.key,
        )
        .await?;

        let mut link = RepoLink {
            repo_key: ctx.repo.key.clone(),
            worktree_path: ctx.worktree_path.to_path_buf(),
            setup_script_ran_at: None,
            github: None,
            attached_prs: Vec::new(),
            created_branch,
        };

        if let Some(script) = ctx
            .repo
            .default_setup_script
            .as_ref()
            .filter(|s| !s.trim().is_empty())
        {
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

        Ok::<RepoLink, AppError>(link)
    }
    .await;

    match provisioned {
        Ok(link) => Ok(link),
        Err(e) => {
            teardown_repo_worktree(RepoTeardown {
                repo_key: &ctx.repo.key,
                worktree_path: ctx.worktree_path,
                branch: ctx.branch,
                created_branch,
                paths: ctx.paths,
                tx: ctx.tx,
            })
            .await;
            Err(e)
        }
    }
}

/// Copy each entry in `copy_files` from the base clone into the new worktree.
/// These are typically gitignored files (`.env`, etc.) that `git worktree add`
/// won't carry over but setup scripts and dev servers need. Missing sources
/// are silently skipped; an existing file at the destination is left alone.
/// Paths must be relative and free of `..` segments — anything else is
/// rejected to keep the copy contained inside `clone_path` / `worktree_path`.
async fn copy_configured_files(
    clone_path: &Path,
    worktree_path: &Path,
    copy_files: &[String],
    tx: &JobTx,
    repo_key: &str,
) -> AppResult<()> {
    for rel in copy_files {
        let rel_path = Path::new(rel);
        if rel_path.is_absolute()
            || rel_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(AppError::Other(format!(
                "copy_files entry '{rel}' must be a relative path without '..' segments",
            )));
        }

        let src = clone_path.join(rel_path);
        match tokio::fs::symlink_metadata(&src).await {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(AppError::Io(e)),
        }

        let dst = worktree_path.join(rel_path);
        if tokio::fs::try_exists(&dst).await? {
            continue;
        }
        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::copy(&src, &dst).await?;
        tx.status(format!("copied {rel} from clone"), Some(repo_key));
    }
    Ok(())
}

struct RepoTeardown<'a> {
    repo_key: &'a str,
    worktree_path: &'a Path,
    branch: &'a str,
    /// Whether Tethys created this branch. A pre-existing branch (e.g. a PR
    /// branch checked out for local edits) is left intact — only the worktree
    /// is removed.
    created_branch: bool,
    paths: &'a Paths,
    tx: &'a JobTx,
}

/// Best-effort reverse of `provision_repo_worktree`: force-remove the
/// worktree, prune stale registrations, and delete the branch when Tethys
/// created it. Errors are streamed as status events but never bubbled —
/// teardown is always best-effort.
async fn teardown_repo_worktree(ctx: RepoTeardown<'_>) {
    if ctx.worktree_path.exists() {
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
        if ctx.created_branch {
            git::branch_delete_best_effort(&clone_path, ctx.branch, ctx.tx, ctx.repo_key).await;
        }
    }
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
        script_runs: Vec::new(),
        notes: String::new(),
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

    // Provisioned links accumulate here so the rollback path can tear down
    // exactly what succeeded (each carries whether Tethys created its branch).
    // A failing repo self-cleans inside `provision_repo_worktree`, so it never
    // appears here.
    let mut created: Vec<RepoLink> = Vec::new();
    let orchestrate = async {
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
        Ok::<_, AppError>(())
    }
    .await;

    match orchestrate {
        Ok(()) => {
            let stored = store
                .mutate(|s| {
                    let ws = s
                        .find_workspace_mut(&id)
                        .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;
                    ws.repo_links = created.clone();
                    ws.status = WorkspaceStatus::Ready;
                    Ok(ws.clone())
                })
                .await?;

            regen_workspace_root_settings(&stored, &paths, &tx).await;
            regen_workspace_claude_md(&stored, reg, &paths, &tx).await;

            info!(id = %stored.id, branch = %stored.branch, repos = stored.repo_links.len(), "created workspace");
            let _ = tx.0.send(JobEvent::Success);
            emit_workspace_changed(&app, &stored.id);
            Ok(stored)
        }
        Err(e) => {
            let msg = e.to_string();
            warn!(error = %msg, "workspace create failed; rolling back worktrees");
            tx.status(format!("tearing down partial workspace: {msg}"), None);

            // Best-effort teardown of the repos we fully provisioned. Each link
            // records whether Tethys created its branch, so a pre-existing
            // branch we merely checked out (e.g. a PR branch) is left intact.
            for link in created.iter().rev() {
                teardown_repo_worktree(RepoTeardown {
                    repo_key: &link.repo_key,
                    worktree_path: &link.worktree_path,
                    branch: &branch,
                    created_branch: link.created_branch,
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
            emit_workspace_changed(&app, &args.workspace_id);
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
        .read(|s| {
            s.find_workspace(&id)
                .map(|w| w.sessions.iter().map(|m| m.id.clone()).collect())
        })
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(id.clone()))?;

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
        SpawnOpts::default(),
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
        SpawnOpts {
            resume_claude_sid: Some(&claude_sid),
            session_binary: session_binary.as_deref(),
        },
    )
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
    app: AppHandle,
    supervisor: State<'_, Arc<SessionSupervisor>>,
    store: State<'_, Arc<Store>>,
    claude_bin: State<'_, ClaudeBin>,
    tmux_bin: State<'_, TmuxBin>,
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

    let start = StartClaudeArgs {
        workspace_id: args.workspace_id,
        repo_key,
    };
    spawn_claude(
        &app,
        &supervisor,
        &store,
        &claude_bin,
        &tmux_bin,
        &start,
        SpawnOpts {
            resume_claude_sid: resume_sid,
            session_binary: Some(&binary),
        },
    )
    .await
}

/// Optional overrides applied when (re)spawning a claude session.
#[derive(Default)]
struct SpawnOpts<'a> {
    /// Resume an existing conversation via `claude --resume <id>`.
    resume_claude_sid: Option<&'a str>,
    /// Per-session binary override to run under and persist onto the new meta.
    /// Takes precedence over the workspace default; `None` falls back to it.
    session_binary: Option<&'a str>,
}

async fn spawn_claude(
    app: &AppHandle,
    supervisor: &Arc<SessionSupervisor>,
    store: &Arc<Store>,
    claude_bin: &ClaudeBin,
    tmux_bin: &TmuxBin,
    args: &StartClaudeArgs,
    opts: SpawnOpts<'_>,
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

    // Session override wins over the workspace default, which wins over the
    // app-wide binary resolved at boot.
    let resolved_bin = match opts.session_binary.or(ws_binary.as_deref()) {
        Some(bin) => claude::resolve_named(bin)?,
        None => claude_bin.0.clone(),
    };

    let (info, _token) = supervisor.spawn_claude(
        args.workspace_id.clone(),
        args.repo_key.clone(),
        &cwd,
        &tmux_bin.0,
        &resolved_bin,
        opts.resume_claude_sid,
    )?;

    // Persist a ClaudeSessionMeta entry so resume works across restarts.
    // claude_session_id is filled in by the SessionStart hook once it
    // arrives. We key on the Tethys-internal `id` (== SessionSupervisor id)
    // so the UI and supervisor use a shared identifier.
    let meta = ClaudeSessionMeta {
        id: info.id.clone(),
        repo_key: args.repo_key.clone(),
        cwd: cwd.clone(),
        claude_session_id: None,
        transcript_path: None,
        claude_binary: opts.session_binary.map(str::to_string),
        hidden: false,
        runtime_state: None,
        notification_type: None,
        turn_acknowledged: false,
    };

    store
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&args.workspace_id)
                .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;
            // Resuming? Drop the prior meta for this Claude conversation so
            // we don't accumulate dormant duplicates with the same
            // claude_session_id across runs.
            if let Some(csid) = opts.resume_claude_sid {
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
        .mutate(|s| {
            let ws = s
                .find_workspace_mut(&args.workspace_id)
                .ok_or_else(|| AppError::WorkspaceNotFound(args.workspace_id.clone()))?;
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
            s.find_workspace(&args.workspace_id).and_then(|w| {
                w.repo_links
                    .iter()
                    .find(|r| r.repo_key == args.repo_key)
                    .map(|r| r.worktree_path.clone())
            })
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
            "failed to seed workspace-root settings.local.json"
        );
        tx.status(
            format!("workspace-root settings seed failed: {e}"),
            None,
        );
    }
}

/// Extend an existing workspace-root settings.local.json with the entries
/// of a newly-added repo. Best-effort.
async fn append_repo_to_workspace_root_settings(
    workspace: &Workspace,
    repo_key: &str,
    paths: &Paths,
    tx: &JobTx,
) {
    let Some(workspace_root) = workspace
        .repo_links
        .first()
        .and_then(|r| r.worktree_path.parent().map(|p| p.to_path_buf()))
    else {
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
