//! Managed Docs: per-repo `CONTEXT.md` + `docs/adr` that the user's team
//! hasn't adopted, so they can't live in the repo itself. Tethys stores them
//! in a Docs Repo per `repo_key`, gives each workspace its own branch + Docs
//! Checkout symlinked into the user worktree, snapshots changes at purge, and
//! parks any diff against docs `main` as a Pending Docs Merge for the user to
//! approve or decline.
//!
//! All Docs Repo git plumbing and the pending-merges store live here. Git ops
//! run silently via `tokio::process::Command`; a few generic helpers from
//! `git.rs` (worktree remove/prune) are reused since they're repo-agnostic.

use std::ffi::OsStr;
use std::path::Path;
use std::pin::Pin;
use std::process::Output;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::warn;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::git;
use crate::job::JobTx;
use crate::paths::Paths;
use crate::registry::{self, Repo, RepoRegistry};
use crate::state::{DocsLink, RepoLink, Workspace};

/// The repo-root-relative paths Tethys manages. The Docs Repo mirrors these at
/// its own root (`<docs_repo>/CONTEXT.md`, `<docs_repo>/docs/adr/`).
const MANAGED_PATHS: [&str; 2] = ["CONTEXT.md", "docs/adr"];

/// `git -C <dir> <args...>`, silent. Returns the raw `Output` so callers can
/// inspect status/stdout (e.g. `diff --quiet`, "nothing to commit").
async fn run_git<I, S>(dir: &Path, args: I) -> AppResult<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::Other(format!("git in {}: {e}", dir.display())))
}

/// Like [`run_git`] but errors on a non-zero exit, folding stderr into the
/// message. For git ops where a failure should abort the operation.
async fn run_git_checked<I, S>(dir: &Path, args: I) -> AppResult<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<std::ffi::OsString> =
        args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
    let output = run_git(dir, &args).await?;
    if !output.status.success() {
        let display: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        return Err(AppError::Other(format!(
            "git {} in {} exited with {:?}: {}",
            display.join(" "),
            dir.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output)
}

/// Whether `<data_dir>/docs/<repo_key>` is a git repo with a resolvable HEAD.
async fn is_valid_docs_repo(repo: &Path) -> bool {
    match run_git(repo, ["rev-parse", "--verify", "HEAD"]).await {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// Ensure the Docs Repo for `repo_key` exists: `git init -b main`, local
/// `Tethys` identity, and an empty initial commit. Idempotent. Never seeds
/// content files — lazy creation is a hard requirement.
pub async fn ensure_docs_repo(paths: &Paths, repo_key: &str) -> AppResult<()> {
    let repo = paths.docs_repo_path(repo_key);
    if is_valid_docs_repo(&repo).await {
        return Ok(());
    }
    fs::create_dir_all(&repo).await?;

    let init = tokio::process::Command::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(&repo)
        .output()
        .await
        .map_err(|e| AppError::Other(format!("git init: {e}")))?;
    if !init.status.success() {
        return Err(AppError::Other(format!(
            "git init -b main in {} failed: {}",
            repo.display(),
            String::from_utf8_lossy(&init.stderr).trim()
        )));
    }

    run_git_checked(&repo, ["config", "user.name", "Tethys"]).await?;
    run_git_checked(&repo, ["config", "user.email", "tethys@localhost"]).await?;
    run_git_checked(&repo, ["commit", "--allow-empty", "-m", "init"]).await?;
    Ok(())
}

/// Inputs for [`provision`].
pub struct DocsProvision<'a> {
    pub repo: &'a Repo,
    pub workspace_id: &'a str,
    pub branch: &'a str,
    pub worktree_path: &'a Path,
    pub paths: &'a Paths,
    pub tx: &'a JobTx,
}

/// Whether the user repo's own git history tracks `path` (the Team-Adopted
/// signal). `git ls-files -- <path>` produces output for both a tracked file
/// and any tracked file under a tracked directory.
async fn is_team_adopted(worktree: &Path, path: &str) -> AppResult<bool> {
    let out = run_git(worktree, ["ls-files", "--", path]).await?;
    Ok(!out.stdout.is_empty())
}

/// Provision Managed Docs for one repo link. Returns the `DocsLink` to store
/// on the link, or `None` when docs are skipped (opt-out or fully
/// Team-Adopted). Failures bubble; the caller's rollback tears down.
pub async fn provision(ctx: DocsProvision<'_>) -> AppResult<Option<DocsLink>> {
    if !ctx.repo.managed_docs {
        return Ok(None);
    }

    let repo_key = &ctx.repo.key;

    // Team-Adopted check per path. A path the user repo tracks is off-limits.
    let mut adopted = [false; MANAGED_PATHS.len()];
    for (i, path) in MANAGED_PATHS.iter().enumerate() {
        adopted[i] = is_team_adopted(ctx.worktree_path, path).await?;
    }
    if adopted.iter().all(|&a| a) {
        ctx.tx.status(
            "managed docs disabled: repo tracks CONTEXT.md and docs/adr",
            Some(repo_key),
        );
        return Ok(None);
    }
    for (i, path) in MANAGED_PATHS.iter().enumerate() {
        if adopted[i] {
            ctx.tx.status(
                format!("managed docs: {path} is team-adopted; leaving it to the repo"),
                Some(repo_key),
            );
        }
    }

    ensure_docs_repo(ctx.paths, repo_key).await?;

    let workspace_dir = registry::sanitize_branch_for_dir(ctx.branch);
    let short_id: String = ctx.workspace_id.chars().take(8).collect();
    let docs_branch = format!("ws/{workspace_dir}-{short_id}");
    let checkout_path = ctx.paths.docs_checkout_path(&workspace_dir, repo_key);
    let docs_repo = ctx.paths.docs_repo_path(repo_key);

    if let Some(parent) = checkout_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    run_git_checked(
        &docs_repo,
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-b"),
            OsStr::new(&docs_branch),
            checkout_path.as_os_str(),
            OsStr::new("main"),
        ],
    )
    .await?;

    // Lazily symlink each non-adopted managed path that exists in the checkout
    // into the user worktree, unless something is already there.
    let mut linked_paths = Vec::new();
    for (i, path) in MANAGED_PATHS.iter().enumerate() {
        if adopted[i] {
            continue;
        }
        let src = checkout_path.join(path);
        if !fs::try_exists(&src).await.unwrap_or(false) {
            continue;
        }
        let dst = ctx.worktree_path.join(path);

        // `docs/adr` needs its parent `docs` dir to exist in the worktree.
        if let Some(parent) = dst.parent() {
            if parent != ctx.worktree_path {
                fs::create_dir_all(parent).await?;
            }
        }

        match fs::symlink_metadata(&dst).await {
            Ok(_) => {
                ctx.tx.status(
                    format!("managed docs: {path} already present in worktree; leaving it (adoption at purge)"),
                    Some(repo_key),
                );
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::Io(e)),
        }

        fs::symlink(&src, &dst).await?;
        ctx.tx.status(
            format!("managed docs: linked {path} -> docs checkout"),
            Some(repo_key),
        );
        linked_paths.push(path.to_string());
    }

    // Hide the managed paths from the user repo's git status via the clone's
    // shared `.git/info/exclude` — but only for non-adopted paths.
    let clone_path = ctx.paths.repo_clone_path(repo_key);
    let exclude_lines: Vec<String> = MANAGED_PATHS
        .iter()
        .enumerate()
        .filter(|(i, _)| !adopted[*i])
        .map(|(_, p)| format!("/{p}"))
        .collect();
    append_exclude_lines(&clone_path, &exclude_lines).await?;

    Ok(Some(DocsLink {
        branch: docs_branch,
        checkout_path,
        linked_paths,
    }))
}

/// Idempotently append lines to `<clone>/.git/info/exclude`, skipping any
/// already present. Creates the file if missing.
async fn append_exclude_lines(clone_path: &Path, lines: &[String]) -> AppResult<()> {
    if lines.is_empty() {
        return Ok(());
    }
    let exclude = clone_path.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude.parent() {
        fs::create_dir_all(parent).await?;
    }
    let existing = match fs::read_to_string(&exclude).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(AppError::Io(e)),
    };
    let present: std::collections::HashSet<&str> =
        existing.lines().map(str::trim).collect();

    let mut to_add = String::new();
    for line in lines {
        if !present.contains(line.as_str()) {
            to_add.push_str(line);
            to_add.push('\n');
        }
    }
    if to_add.is_empty() {
        return Ok(());
    }

    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&to_add);
    fs::write(&exclude, content).await?;
    Ok(())
}

/// Best-effort teardown of a workspace's Docs Checkout AND its branch. Used on
/// rollback where the whole provisioning is being undone. Never errors.
pub async fn remove_checkout_and_branch(paths: &Paths, repo_key: &str, docs: &DocsLink) {
    let docs_repo = paths.docs_repo_path(repo_key);
    remove_checkout(&docs_repo, &docs.checkout_path).await;
    git::branch_delete_best_effort_silent(&docs_repo, &docs.branch).await;
}

/// Best-effort removal of just the Docs Checkout worktree (the branch
/// survives): `worktree remove --force`, `worktree prune`, and cleanup of the
/// now-possibly-empty `<data_dir>/docs-worktrees/<workspace_dir>` parent.
async fn remove_checkout(docs_repo: &Path, checkout_path: &Path) {
    let _ = git::worktree_remove_silent(docs_repo, checkout_path, true).await;
    git::worktree_prune_best_effort_silent(docs_repo).await;
    if let Some(parent) = checkout_path.parent() {
        // Only removes it if empty; a shared parent with sibling repos stays.
        let _ = fs::remove_dir(parent).await;
    }
}

/// Take the Snapshot for a purging workspace: adopt any real docs files a
/// session created, commit the workspace's docs branch, remove the checkout,
/// then either delete an unchanged branch or park a Pending Docs Merge.
///
/// Called from `purge_workspace` BEFORE any worktree removal. Errors bubble
/// and abort the purge — a failed Snapshot followed by worktree removal would
/// silently destroy the user's docs edits.
///
/// Idempotent across the purger's hourly retries: a link whose branch is
/// already gone (finalized on a prior attempt, or approved/declined since) is
/// skipped, and an entry is never appended twice for the same
/// `(repo_key, docs_branch)`.
///
/// `registry` is used only for retro-adoption of pre-feature links
/// (`docs: None`); a `None` registry (broken `repos.toml`) skips retro-adoption
/// rather than blocking the purge.
pub async fn snapshot_for_purge(
    workspace: &Workspace,
    paths: &Paths,
    registry: Option<&RepoRegistry>,
) -> AppResult<()> {
    let path = paths.pending_docs_merges_file();
    let mut file = load_file(&path).await?;
    let mut changed = false;

    for link in &workspace.repo_links {
        let entry = match &link.docs {
            Some(docs) => snapshot_provisioned_link(workspace, paths, link, docs).await?,
            None => retro_snapshot_link(workspace, paths, registry, link).await?,
        };
        let Some(entry) = entry else { continue };
        // Dedup against prior attempts (and sibling links that resolved to the
        // same branch name in the same Docs Repo).
        let dup = file
            .entries
            .iter()
            .any(|e| e.repo_key == entry.repo_key && e.docs_branch == entry.docs_branch);
        if !dup {
            file.entries.push(entry);
            changed = true;
        }
    }

    if changed {
        save_file(&path, &file).await?;
    }
    Ok(())
}

/// Snapshot a link that was provisioned with Managed Docs. Returns the entry
/// to park, or `None` when the branch is already gone (prior attempt / approved
/// / declined) or the Snapshot produced no diff.
async fn snapshot_provisioned_link(
    workspace: &Workspace,
    paths: &Paths,
    link: &RepoLink,
    docs: &DocsLink,
) -> AppResult<Option<PendingDocsMerge>> {
    let docs_repo = paths.docs_repo_path(&link.repo_key);
    // The branch is the durable record that a Snapshot still needs taking. If
    // it's gone, a prior purge attempt already finalized this link (or the user
    // approved/declined it) — nothing left to do.
    if !git::branch_exists(&docs_repo, &docs.branch).await? {
        return Ok(None);
    }

    if fs::try_exists(&docs.checkout_path).await.unwrap_or(false) {
        adopt_worktree_docs(&link.worktree_path, &docs.checkout_path).await?;
    }
    finalize_snapshot(
        workspace,
        &docs_repo,
        &docs.branch,
        &docs.checkout_path,
        &link.repo_key,
    )
    .await
}

/// Retro-adoption for a pre-feature link (`docs: None`): if the user worktree
/// holds real, untracked `CONTEXT.md` / `docs/adr` files, capture them into a
/// fresh docs branch so the imminent worktree removal doesn't destroy them.
/// Returns `None` when retro-adoption doesn't apply (opt-out, no registry, no
/// adoptable files) or the Snapshot produced no diff.
async fn retro_snapshot_link(
    workspace: &Workspace,
    paths: &Paths,
    registry: Option<&RepoRegistry>,
    link: &RepoLink,
) -> AppResult<Option<PendingDocsMerge>> {
    let Some(reg) = registry else {
        return Ok(None);
    };
    let Some(repo) = reg.find_repo(&link.repo_key) else {
        return Ok(None);
    };
    if !repo.managed_docs {
        return Ok(None);
    }
    if !fs::try_exists(&link.worktree_path).await.unwrap_or(false) {
        return Ok(None);
    }

    let mut has_adoptable = false;
    for path in MANAGED_PATHS {
        if is_adoptable(&link.worktree_path, path).await? {
            has_adoptable = true;
            break;
        }
    }
    if !has_adoptable {
        return Ok(None);
    }

    ensure_docs_repo(paths, &link.repo_key).await?;
    let docs_repo = paths.docs_repo_path(&link.repo_key);
    let workspace_dir = registry::sanitize_branch_for_dir(&workspace.branch);
    let short_id: String = workspace.id.chars().take(8).collect();
    let docs_branch = format!("ws/{workspace_dir}-{short_id}");
    let checkout = paths.docs_checkout_path(&workspace_dir, &link.repo_key);

    if !git::branch_exists(&docs_repo, &docs_branch).await? {
        if let Some(parent) = checkout.parent() {
            fs::create_dir_all(parent).await?;
        }
        run_git_checked(
            &docs_repo,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("-b"),
                OsStr::new(&docs_branch),
                checkout.as_os_str(),
                OsStr::new("main"),
            ],
        )
        .await?;
    }
    // Re-adopt whenever the checkout is still around, so a retry after a
    // mid-adoption failure picks up the files the first attempt missed. A
    // completed prior attempt removed the checkout, leaving the branch as the
    // durable record — finalize handles that (dedup keeps the entry unique).
    if fs::try_exists(&checkout).await.unwrap_or(false) {
        adopt_worktree_docs(&link.worktree_path, &checkout).await?;
    }

    finalize_snapshot(workspace, &docs_repo, &docs_branch, &checkout, &link.repo_key).await
}

/// Shared tail of the Snapshot: commit the checkout (if present), remove it,
/// then diff the branch against its merge-base with `main` — delete an
/// unchanged branch silently, or build a `PendingDocsMerge` for a changed one.
async fn finalize_snapshot(
    workspace: &Workspace,
    docs_repo: &Path,
    branch: &str,
    checkout: &Path,
    repo_key: &str,
) -> AppResult<Option<PendingDocsMerge>> {
    if fs::try_exists(checkout).await.unwrap_or(false) {
        run_git_checked(checkout, ["add", "-A"]).await?;
        // `diff --cached --quiet` exits non-zero when there's something staged;
        // commit only then so "nothing to commit" is success.
        let staged = run_git(checkout, ["diff", "--cached", "--quiet"]).await?;
        if !staged.status.success() {
            let msg = format!("snapshot from workspace {}", workspace.branch);
            run_git_checked(checkout, ["commit", "-m", &msg]).await?;
        }
        remove_checkout(docs_repo, checkout).await;
    } else {
        warn!(
            repo = %repo_key,
            checkout = %checkout.display(),
            "docs checkout missing at purge; using branch as-is"
        );
    }

    // Any-diff test against the merge-base (main may have advanced via other
    // merges, so its tip is the wrong reference).
    let base_out = run_git_checked(docs_repo, ["merge-base", "main", branch]).await?;
    let base = String::from_utf8_lossy(&base_out.stdout).trim().to_string();

    let diff_quiet = run_git(docs_repo, ["diff", "--quiet", &base, branch]).await?;
    if diff_quiet.status.success() {
        // No diff — drop the branch silently.
        git::branch_delete_best_effort_silent(docs_repo, branch).await;
        return Ok(None);
    }

    let diff_out = run_git_checked(docs_repo, ["diff", &base, branch]).await?;
    let diff = String::from_utf8_lossy(&diff_out.stdout).into_owned();
    Ok(Some(PendingDocsMerge {
        id: Uuid::new_v4().to_string(),
        repo_key: repo_key.to_string(),
        workspace_id: workspace.id.clone(),
        workspace_branch: workspace.branch.clone(),
        docs_branch: branch.to_string(),
        captured_at: Utc::now(),
        diff,
        conflicted: false,
        conflict_files: Vec::new(),
    }))
}

/// Whether a managed `path` in the user worktree should be adopted: it exists,
/// is a real (non-symlink) file/dir, and is NOT Team-Adopted (the repo's own
/// git history tracking it makes the team's version canonical).
async fn is_adoptable(worktree: &Path, path: &str) -> AppResult<bool> {
    let wt_path = worktree.join(path);
    let meta = match fs::symlink_metadata(&wt_path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(AppError::Io(e)),
    };
    if meta.file_type().is_symlink() {
        return Ok(false);
    }
    if is_team_adopted(worktree, path).await? {
        return Ok(false);
    }
    Ok(true)
}

/// Adoption: for each adoptable managed path (see [`is_adoptable`]), replace
/// the checkout's version with the worktree's real file/dir. Symlinked and
/// Team-Adopted paths are left alone — the former already lives in the checkout,
/// the latter belongs to the team's repo and must never be shadowed.
async fn adopt_worktree_docs(worktree: &Path, checkout: &Path) -> AppResult<()> {
    for path in MANAGED_PATHS {
        if !is_adoptable(worktree, path).await? {
            continue;
        }
        let wt_path = worktree.join(path);
        let meta = fs::symlink_metadata(&wt_path).await?;

        let dst = checkout.join(path);
        remove_path(&dst).await?;
        if meta.is_dir() {
            copy_dir_recursive(&wt_path, &dst).await?;
        } else {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::copy(&wt_path, &dst).await?;
        }
    }
    Ok(())
}

/// Remove a file or directory if it exists; no-op if missing.
async fn remove_path(p: &Path) -> AppResult<()> {
    match fs::symlink_metadata(p).await {
        Ok(meta) => {
            if meta.is_dir() {
                fs::remove_dir_all(p).await?;
            } else {
                fs::remove_file(p).await?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::Io(e)),
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Recursively copy `src` into `dst`, following any symlinks so the result is
/// real content (adoption wants the concrete files a session wrote).
fn copy_dir_recursive<'a>(src: &'a Path, dst: &'a Path) -> BoxFuture<'a, AppResult<()>> {
    Box::pin(async move {
        fs::create_dir_all(dst).await?;
        let mut entries = fs::read_dir(src).await?;
        while let Some(entry) = entries.next_entry().await? {
            let from = entry.path();
            let to = dst.join(entry.file_name());
            let ft = entry.file_type().await?;
            if ft.is_dir() {
                copy_dir_recursive(&from, &to).await?;
            } else {
                // `fs::copy` follows symlinks, yielding concrete file content.
                fs::copy(&from, &to).await?;
            }
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDocsMerge {
    pub id: String,
    pub repo_key: String,
    pub workspace_id: String,
    pub workspace_branch: String,
    pub docs_branch: String,
    pub captured_at: DateTime<Utc>,
    /// Unified diff (merge-base → branch) captured at Snapshot time.
    pub diff: String,
    #[serde(default)]
    pub conflicted: bool,
    #[serde(default)]
    pub conflict_files: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingDocsMergesFile {
    #[serde(default)]
    pub entries: Vec<PendingDocsMerge>,
}

pub async fn load_file(path: &Path) -> AppResult<PendingDocsMergesFile> {
    match fs::read_to_string(path).await {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| AppError::Other(format!("parsing pending_docs_merges.json: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(PendingDocsMergesFile::default())
        }
        Err(e) => Err(AppError::Io(e)),
    }
}

async fn save_file(path: &Path, file: &PendingDocsMergesFile) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let content = serde_json::to_string_pretty(file)
        .map_err(|e| AppError::Other(format!("serializing pending_docs_merges.json: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, path).await?;
    Ok(())
}

/// The branch currently checked out in `dir`, or `None` if detached/unreadable.
async fn current_branch(dir: &Path) -> Option<String> {
    let out = run_git(dir, ["symbolic-ref", "--short", "HEAD"]).await.ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Approve a Pending Docs Merge: merge its branch into docs `main`. On a clean
/// merge the branch is deleted and the entry removed. On conflict the merge is
/// aborted, the entry is flagged `conflicted` (with the conflicting files) and
/// retained, and an error naming the files + docs repo path is returned.
pub async fn approve(paths: &Paths, id: &str) -> AppResult<()> {
    let path = paths.pending_docs_merges_file();
    let mut file = load_file(&path).await?;
    let idx = file
        .entries
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| AppError::Other(format!("pending docs merge '{id}' not found")))?;

    let repo_key = file.entries[idx].repo_key.clone();
    let docs_branch = file.entries[idx].docs_branch.clone();
    let docs_repo = paths.docs_repo_path(&repo_key);

    if current_branch(&docs_repo).await.as_deref() != Some("main") {
        run_git_checked(&docs_repo, ["checkout", "main"]).await?;
    }

    let msg = format!("merge {docs_branch}");
    let merge = run_git(&docs_repo, ["merge", &docs_branch, "-m", &msg]).await?;

    if merge.status.success() {
        git::branch_delete_best_effort_silent(&docs_repo, &docs_branch).await;
        file.entries.remove(idx);
        save_file(&path, &file).await?;
        return Ok(());
    }

    // Conflict: collect unmerged files before aborting, then abort.
    let unmerged = run_git(&docs_repo, ["diff", "--name-only", "--diff-filter=U"]).await?;
    let files: Vec<String> = String::from_utf8_lossy(&unmerged.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let _ = run_git(&docs_repo, ["merge", "--abort"]).await;

    let entry = &mut file.entries[idx];
    entry.conflicted = true;
    entry.conflict_files = files.clone();
    save_file(&path, &file).await?;

    Err(AppError::Other(format!(
        "docs merge of {docs_branch} conflicted in: {}. Resolve manually in {}",
        if files.is_empty() {
            "(unknown files)".to_string()
        } else {
            files.join(", ")
        },
        docs_repo.display()
    )))
}

/// Decline a Pending Docs Merge: rename its branch to `archive/<name>` (never
/// hard-delete) and remove the entry. `ws/feat-foo-1a2b` becomes
/// `archive/feat-foo-1a2b`, suffixed `-2`, `-3`, … on collision.
pub async fn decline(paths: &Paths, id: &str) -> AppResult<()> {
    let path = paths.pending_docs_merges_file();
    let mut file = load_file(&path).await?;
    let idx = file
        .entries
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| AppError::Other(format!("pending docs merge '{id}' not found")))?;

    let repo_key = file.entries[idx].repo_key.clone();
    let docs_branch = file.entries[idx].docs_branch.clone();
    let docs_repo = paths.docs_repo_path(&repo_key);

    let stem = docs_branch.strip_prefix("ws/").unwrap_or(&docs_branch);
    let mut target = format!("archive/{stem}");
    let mut n = 2;
    while git::branch_exists(&docs_repo, &target).await? {
        target = format!("archive/{stem}-{n}");
        n += 1;
    }
    run_git_checked(&docs_repo, ["branch", "-m", &docs_branch, &target]).await?;

    file.entries.remove(idx);
    save_file(&path, &file).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{RepoLink, WorkspaceStatus};
    use std::process::Command as StdCommand;

    fn noop_tx() -> JobTx {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        JobTx(tx)
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let out = StdCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} in {} failed: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Initialize `clone` as a user repo on `main` with one commit and local
    /// identity (so tests don't depend on global git config).
    fn init_user_clone(clone: &Path) {
        let out = StdCommand::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(clone)
            .output()
            .expect("spawn git init");
        assert!(out.status.success(), "git init failed");
        git_ok(clone, &["config", "user.email", "test@example.com"]);
        git_ok(clone, &["config", "user.name", "Test"]);
        std::fs::write(clone.join("README.md"), "hi").unwrap();
        git_ok(clone, &["add", "."]);
        git_ok(clone, &["commit", "-m", "init"]);
    }

    /// Add a user worktree on a fresh branch off the clone's main.
    fn add_user_worktree(clone: &Path, worktree: &Path, branch: &str) {
        git_ok(
            clone,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                worktree.to_str().unwrap(),
                "main",
            ],
        );
    }

    fn test_repo(key: &str) -> Repo {
        Repo {
            key: key.to_string(),
            remote_url: "git@example.com:me/repo.git".to_string(),
            default_branch: None,
            default_setup_script: None,
            setup_timeout_secs: None,
            copy_files: Vec::new(),
            scripts: Default::default(),
            managed_docs: true,
            github_slug: None,
        }
    }

    fn make_workspace(id: &str, branch: &str, links: Vec<RepoLink>) -> Workspace {
        Workspace {
            id: id.to_string(),
            branch: branch.to_string(),
            created_at: Utc::now(),
            repo_links: links,
            sessions: Vec::new(),
            claude_binary: None,
            deleted_at: None,
            archived_at: None,
            status: WorkspaceStatus::Ready,
            script_runs: Vec::new(),
            notes: String::new(),
        }
    }

    /// Commit `content` at repo-relative `path` on the Docs Repo's `main`.
    async fn commit_docs_main(paths: &Paths, repo_key: &str, rel: &str, content: &str) {
        let repo = paths.docs_repo_path(repo_key);
        let full = repo.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).await.unwrap();
        }
        fs::write(&full, content).await.unwrap();
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-m", &format!("add {rel}")]);
    }

    #[tokio::test]
    async fn ensure_docs_repo_is_idempotent_and_commitable() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        ensure_docs_repo(&paths, "repo").await.unwrap();
        let repo = paths.docs_repo_path("repo");
        assert!(is_valid_docs_repo(&repo).await);
        // No content seeded.
        assert!(!repo.join("CONTEXT.md").exists());

        // Idempotent.
        ensure_docs_repo(&paths, "repo").await.unwrap();

        // The repo can take a commit (identity was configured).
        fs::write(repo.join("x.txt"), "hi").await.unwrap();
        run_git_checked(&repo, ["add", "-A"]).await.unwrap();
        run_git_checked(&repo, ["commit", "-m", "c"]).await.unwrap();
    }

    #[tokio::test]
    async fn provision_empty_docs_repo_creates_branch_no_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        let worktree = tmp.path().join("wt/feat-foo/repo");
        add_user_worktree(&clone, &worktree, "feat/foo");

        let repo = test_repo("repo");
        let docs = provision(DocsProvision {
            repo: &repo,
            workspace_id: "abcd1234ffff",
            branch: "feat/foo",
            worktree_path: &worktree,
            paths: &paths,
            tx: &noop_tx(),
        })
        .await
        .unwrap()
        .expect("docs provisioned");

        assert_eq!(docs.branch, "ws/feat-foo-abcd1234");
        assert!(docs.linked_paths.is_empty(), "lazy: nothing to symlink");
        assert!(docs.checkout_path.exists());
        // No symlink in the worktree.
        assert!(!worktree.join("CONTEXT.md").exists());

        // Exclude entries written.
        let exclude = fs::read_to_string(clone.join(".git/info/exclude"))
            .await
            .unwrap();
        assert!(exclude.lines().any(|l| l == "/CONTEXT.md"));
        assert!(exclude.lines().any(|l| l == "/docs/adr"));

        // A second provision for a different workspace works alongside.
        let worktree2 = tmp.path().join("wt/feat-bar/repo");
        add_user_worktree(&clone, &worktree2, "feat/bar");
        let docs2 = provision(DocsProvision {
            repo: &repo,
            workspace_id: "99998888aaaa",
            branch: "feat/bar",
            worktree_path: &worktree2,
            paths: &paths,
            tx: &noop_tx(),
        })
        .await
        .unwrap()
        .expect("second docs provisioned");
        assert_eq!(docs2.branch, "ws/feat-bar-99998888");
        assert!(docs2.checkout_path.exists());
    }

    #[tokio::test]
    async fn provision_symlinks_existing_context_md() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        let worktree = tmp.path().join("wt/feat-foo/repo");
        add_user_worktree(&clone, &worktree, "feat/foo");

        ensure_docs_repo(&paths, "repo").await.unwrap();
        commit_docs_main(&paths, "repo", "CONTEXT.md", "hello docs\n").await;

        let repo = test_repo("repo");
        let docs = provision(DocsProvision {
            repo: &repo,
            workspace_id: "abcd1234",
            branch: "feat/foo",
            worktree_path: &worktree,
            paths: &paths,
            tx: &noop_tx(),
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(docs.linked_paths, vec!["CONTEXT.md".to_string()]);
        let link = worktree.join("CONTEXT.md");
        assert!(fs::symlink_metadata(&link).await.unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&link).await.unwrap(), "hello docs\n");
    }

    #[tokio::test]
    async fn team_adopted_paths_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        // The user repo tracks CONTEXT.md — team-adopted for that path.
        std::fs::write(clone.join("CONTEXT.md"), "team owns this\n").unwrap();
        git_ok(&clone, &["add", "CONTEXT.md"]);
        git_ok(&clone, &["commit", "-m", "adopt context"]);

        let worktree = tmp.path().join("wt/feat-foo/repo");
        add_user_worktree(&clone, &worktree, "feat/foo");

        // Docs repo main also has CONTEXT.md, but adoption must win.
        ensure_docs_repo(&paths, "repo").await.unwrap();
        commit_docs_main(&paths, "repo", "CONTEXT.md", "tethys version\n").await;

        let repo = test_repo("repo");
        let docs = provision(DocsProvision {
            repo: &repo,
            workspace_id: "abcd1234",
            branch: "feat/foo",
            worktree_path: &worktree,
            paths: &paths,
            tx: &noop_tx(),
        })
        .await
        .unwrap()
        .expect("docs still provisioned (docs/adr not adopted)");
        assert!(
            !docs.linked_paths.contains(&"CONTEXT.md".to_string()),
            "adopted CONTEXT.md not symlinked"
        );
        // The worktree's committed CONTEXT.md is untouched.
        assert_eq!(
            fs::read_to_string(worktree.join("CONTEXT.md")).await.unwrap(),
            "team owns this\n"
        );

        // Tracking BOTH paths yields Ok(None). Force-add since the first
        // provision already wrote `/docs/adr` into the clone's info/exclude.
        std::fs::create_dir_all(clone.join("docs/adr")).unwrap();
        std::fs::write(clone.join("docs/adr/0001.md"), "adr\n").unwrap();
        git_ok(&clone, &["add", "-f", "docs/adr/0001.md"]);
        git_ok(&clone, &["commit", "-m", "adopt adr"]);
        let worktree2 = tmp.path().join("wt/feat-bar/repo");
        add_user_worktree(&clone, &worktree2, "feat/bar");
        let both = provision(DocsProvision {
            repo: &repo,
            workspace_id: "99998888",
            branch: "feat/bar",
            worktree_path: &worktree2,
            paths: &paths,
            tx: &noop_tx(),
        })
        .await
        .unwrap();
        assert!(both.is_none(), "both paths adopted => None");
    }

    /// Provision a docs-enabled workspace, returning the RepoLink to snapshot.
    async fn provision_link(
        paths: &Paths,
        clone: &Path,
        tmp: &Path,
        workspace_id: &str,
        branch: &str,
    ) -> RepoLink {
        let dir = registry::sanitize_branch_for_dir(branch);
        let worktree = tmp.join(format!("wt/{dir}/repo"));
        add_user_worktree(clone, &worktree, branch);
        let repo = test_repo("repo");
        let docs = provision(DocsProvision {
            repo: &repo,
            workspace_id,
            branch,
            worktree_path: &worktree,
            paths,
            tx: &noop_tx(),
        })
        .await
        .unwrap();
        RepoLink {
            repo_key: "repo".to_string(),
            worktree_path: worktree,
            setup_script_ran_at: None,
            github: None,
            created_branch: true,
            docs,
        }
    }

    #[tokio::test]
    async fn snapshot_commits_symlink_edit_and_parks_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        ensure_docs_repo(&paths, "repo").await.unwrap();
        commit_docs_main(&paths, "repo", "CONTEXT.md", "base\n").await;

        let link = provision_link(&paths, &clone, tmp.path(), "abcd1234", "feat/foo").await;
        assert!(link.docs.as_ref().unwrap().linked_paths.contains(&"CONTEXT.md".to_string()));

        // Edit through the symlink (writes the checkout's file).
        fs::write(link.worktree_path.join("CONTEXT.md"), "edited\n")
            .await
            .unwrap();

        let ws = make_workspace("abcd1234", "feat/foo", vec![link]);
        snapshot_for_purge(&ws, &paths, None).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert_eq!(file.entries.len(), 1);
        let entry = &file.entries[0];
        assert!(!entry.diff.is_empty());
        assert!(entry.diff.contains("edited"));
        assert!(!entry.conflicted);
    }

    #[tokio::test]
    async fn snapshot_adopts_real_context_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        // Empty docs repo — nothing to symlink; session creates a real file.
        let link = provision_link(&paths, &clone, tmp.path(), "abcd1234", "feat/foo").await;
        assert!(link.docs.as_ref().unwrap().linked_paths.is_empty());

        fs::write(link.worktree_path.join("CONTEXT.md"), "created by session\n")
            .await
            .unwrap();

        let ws = make_workspace("abcd1234", "feat/foo", vec![link]);
        snapshot_for_purge(&ws, &paths, None).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert_eq!(file.entries.len(), 1);
        assert!(file.entries[0].diff.contains("created by session"));

        // The adopted file lives on the branch.
        let docs_repo = paths.docs_repo_path("repo");
        let branch = &file.entries[0].docs_branch;
        let show = run_git(&docs_repo, ["show", &format!("{branch}:CONTEXT.md")])
            .await
            .unwrap();
        assert!(show.status.success());
        assert_eq!(String::from_utf8_lossy(&show.stdout), "created by session\n");
    }

    #[tokio::test]
    async fn snapshot_deletes_unchanged_branch_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        let link = provision_link(&paths, &clone, tmp.path(), "abcd1234", "feat/foo").await;
        let docs_repo = paths.docs_repo_path("repo");
        let branch = link.docs.as_ref().unwrap().branch.clone();

        let ws = make_workspace("abcd1234", "feat/foo", vec![link]);
        snapshot_for_purge(&ws, &paths, None).await.unwrap();

        // No pending entry, branch gone.
        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert!(file.entries.is_empty());
        assert!(!git::branch_exists(&docs_repo, &branch).await.unwrap());
    }

    #[tokio::test]
    async fn approve_lands_clean_merge_on_main() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        ensure_docs_repo(&paths, "repo").await.unwrap();
        commit_docs_main(&paths, "repo", "CONTEXT.md", "base\n").await;

        let link = provision_link(&paths, &clone, tmp.path(), "abcd1234", "feat/foo").await;
        fs::write(link.worktree_path.join("CONTEXT.md"), "approved\n")
            .await
            .unwrap();
        let branch = link.docs.as_ref().unwrap().branch.clone();
        let ws = make_workspace("abcd1234", "feat/foo", vec![link]);
        snapshot_for_purge(&ws, &paths, None).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        let id = file.entries[0].id.clone();
        approve(&paths, &id).await.unwrap();

        let docs_repo = paths.docs_repo_path("repo");
        assert_eq!(
            fs::read_to_string(docs_repo.join("CONTEXT.md")).await.unwrap(),
            "approved\n"
        );
        assert!(!git::branch_exists(&docs_repo, &branch).await.unwrap());
        let after = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert!(after.entries.is_empty());
    }

    #[tokio::test]
    async fn approve_conflict_retains_entry_and_leaves_main_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        ensure_docs_repo(&paths, "repo").await.unwrap();
        commit_docs_main(&paths, "repo", "CONTEXT.md", "line1\n").await;

        // Two workspaces both branch from the same base and edit the same line.
        let link_a = provision_link(&paths, &clone, tmp.path(), "aaaa1111", "feat/a").await;
        fs::write(link_a.worktree_path.join("CONTEXT.md"), "A change\n")
            .await
            .unwrap();
        let ws_a = make_workspace("aaaa1111", "feat/a", vec![link_a]);
        snapshot_for_purge(&ws_a, &paths, None).await.unwrap();

        let link_b = provision_link(&paths, &clone, tmp.path(), "bbbb2222", "feat/b").await;
        fs::write(link_b.worktree_path.join("CONTEXT.md"), "B change\n")
            .await
            .unwrap();
        let ws_b = make_workspace("bbbb2222", "feat/b", vec![link_b]);
        snapshot_for_purge(&ws_b, &paths, None).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert_eq!(file.entries.len(), 2);
        let id_a = file.entries[0].id.clone();
        let id_b = file.entries[1].id.clone();

        // First approves clean.
        approve(&paths, &id_a).await.unwrap();
        let docs_repo = paths.docs_repo_path("repo");
        assert_eq!(
            fs::read_to_string(docs_repo.join("CONTEXT.md")).await.unwrap(),
            "A change\n"
        );

        // Second conflicts.
        let err = approve(&paths, &id_b).await.unwrap_err();
        assert!(err.to_string().contains("conflict"));
        // Main untouched (merge aborted).
        assert_eq!(
            fs::read_to_string(docs_repo.join("CONTEXT.md")).await.unwrap(),
            "A change\n"
        );
        // Entry retained + flagged.
        let after = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        let entry_b = after.entries.iter().find(|e| e.id == id_b).expect("b retained");
        assert!(entry_b.conflicted);
        assert!(entry_b.conflict_files.iter().any(|f| f == "CONTEXT.md"));
    }

    #[tokio::test]
    async fn decline_archives_branch_and_removes_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        ensure_docs_repo(&paths, "repo").await.unwrap();
        commit_docs_main(&paths, "repo", "CONTEXT.md", "base\n").await;

        let link = provision_link(&paths, &clone, tmp.path(), "abcd1234", "feat/foo").await;
        fs::write(link.worktree_path.join("CONTEXT.md"), "declined\n")
            .await
            .unwrap();
        let branch = link.docs.as_ref().unwrap().branch.clone();
        let ws = make_workspace("abcd1234", "feat/foo", vec![link]);
        snapshot_for_purge(&ws, &paths, None).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        let id = file.entries[0].id.clone();
        decline(&paths, &id).await.unwrap();

        let docs_repo = paths.docs_repo_path("repo");
        assert!(!git::branch_exists(&docs_repo, &branch).await.unwrap());
        let archived = branch.replacen("ws/", "archive/", 1);
        assert!(git::branch_exists(&docs_repo, &archived).await.unwrap());
        let after = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert!(after.entries.is_empty());
    }

    #[tokio::test]
    async fn exclude_entries_are_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        let repo = test_repo("repo");

        for (wsid, branch) in [("abcd1234", "feat/foo"), ("99998888", "feat/bar")] {
            let dir = registry::sanitize_branch_for_dir(branch);
            let worktree = tmp.path().join(format!("wt/{dir}/repo"));
            add_user_worktree(&clone, &worktree, branch);
            provision(DocsProvision {
                repo: &repo,
                workspace_id: wsid,
                branch,
                worktree_path: &worktree,
                paths: &paths,
                tx: &noop_tx(),
            })
            .await
            .unwrap();
        }

        let exclude = fs::read_to_string(clone.join(".git/info/exclude"))
            .await
            .unwrap();
        assert_eq!(exclude.lines().filter(|l| *l == "/CONTEXT.md").count(), 1);
        assert_eq!(exclude.lines().filter(|l| *l == "/docs/adr").count(), 1);
    }

    fn test_registry(repo: Repo, worktree_root: &Path) -> RepoRegistry {
        RepoRegistry {
            worktree_root: worktree_root.to_path_buf(),
            repos: vec![repo],
        }
    }

    /// A pre-feature (`docs: None`) link over `worktree`.
    fn none_docs_link(worktree: std::path::PathBuf) -> RepoLink {
        RepoLink {
            repo_key: "repo".to_string(),
            worktree_path: worktree,
            setup_script_ran_at: None,
            github: None,
            created_branch: true,
            docs: None,
        }
    }

    #[tokio::test]
    async fn snapshot_twice_diff_case_parks_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        ensure_docs_repo(&paths, "repo").await.unwrap();
        commit_docs_main(&paths, "repo", "CONTEXT.md", "base\n").await;

        let link = provision_link(&paths, &clone, tmp.path(), "abcd1234", "feat/foo").await;
        fs::write(link.worktree_path.join("CONTEXT.md"), "edited\n")
            .await
            .unwrap();
        let ws = make_workspace("abcd1234", "feat/foo", vec![link]);

        snapshot_for_purge(&ws, &paths, None).await.unwrap();
        // A purger retry (purge failed after the snapshot) must succeed and
        // not append a duplicate.
        snapshot_for_purge(&ws, &paths, None).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert_eq!(file.entries.len(), 1);
    }

    #[tokio::test]
    async fn snapshot_twice_no_diff_case_second_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        let link = provision_link(&paths, &clone, tmp.path(), "abcd1234", "feat/foo").await;
        let ws = make_workspace("abcd1234", "feat/foo", vec![link]);

        snapshot_for_purge(&ws, &paths, None).await.unwrap();
        // Branch was deleted on attempt 1; the retry must not fail on a
        // now-missing branch (no merge-base wedge).
        snapshot_for_purge(&ws, &paths, None).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert!(file.entries.is_empty());
    }

    #[tokio::test]
    async fn retro_adoption_captures_untracked_context_md() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        let worktree = tmp.path().join("wt/feat-foo/repo");
        add_user_worktree(&clone, &worktree, "feat/foo");
        // A pre-feature session left a real, untracked CONTEXT.md behind.
        fs::write(worktree.join("CONTEXT.md"), "legacy docs\n")
            .await
            .unwrap();

        let ws = make_workspace("abcd1234", "feat/foo", vec![none_docs_link(worktree)]);
        let reg = test_registry(test_repo("repo"), tmp.path());

        snapshot_for_purge(&ws, &paths, Some(&reg)).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert_eq!(file.entries.len(), 1);
        let branch = file.entries[0].docs_branch.clone();
        assert_eq!(branch, "ws/feat-foo-abcd1234");

        let docs_repo = paths.docs_repo_path("repo");
        let show = run_git(&docs_repo, ["show", &format!("{branch}:CONTEXT.md")])
            .await
            .unwrap();
        assert!(show.status.success());
        assert_eq!(String::from_utf8_lossy(&show.stdout), "legacy docs\n");

        // Idempotent across a purger retry.
        snapshot_for_purge(&ws, &paths, Some(&reg)).await.unwrap();
        let file2 = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert_eq!(file2.entries.len(), 1);
    }

    #[tokio::test]
    async fn retro_adoption_skips_team_adopted_context() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        // The repo tracks CONTEXT.md — team-adopted.
        std::fs::write(clone.join("CONTEXT.md"), "team owns this\n").unwrap();
        git_ok(&clone, &["add", "CONTEXT.md"]);
        git_ok(&clone, &["commit", "-m", "adopt context"]);
        let worktree = tmp.path().join("wt/feat-foo/repo");
        add_user_worktree(&clone, &worktree, "feat/foo");

        let ws = make_workspace("abcd1234", "feat/foo", vec![none_docs_link(worktree)]);
        let reg = test_registry(test_repo("repo"), tmp.path());

        snapshot_for_purge(&ws, &paths, Some(&reg)).await.unwrap();

        // No adoptable paths => no branch, no entry (docs repo never created).
        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert!(file.entries.is_empty());
        assert!(!paths.docs_repo_path("repo").exists());
    }

    #[tokio::test]
    async fn retro_adoption_respects_managed_docs_false() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let clone = paths.repo_clone_path("repo");
        init_user_clone(&clone);
        let worktree = tmp.path().join("wt/feat-foo/repo");
        add_user_worktree(&clone, &worktree, "feat/foo");
        fs::write(worktree.join("CONTEXT.md"), "legacy docs\n")
            .await
            .unwrap();

        let ws = make_workspace("abcd1234", "feat/foo", vec![none_docs_link(worktree)]);
        let mut repo = test_repo("repo");
        repo.managed_docs = false;
        let reg = test_registry(repo, tmp.path());

        snapshot_for_purge(&ws, &paths, Some(&reg)).await.unwrap();

        let file = load_file(&paths.pending_docs_merges_file()).await.unwrap();
        assert!(file.entries.is_empty());
        assert!(!paths.docs_repo_path("repo").exists());
    }
}
