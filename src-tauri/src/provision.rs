//! Provisioning and teardown of one repo's worktree inside a workspace.
//!
//! This lived in the middle of `commands.rs`, which meant the riskiest code in
//! the app — including the rule deciding whether Purge may delete one of the
//! user's own branches — sat between two awaits in a 2000-line module and
//! could only be exercised through the Tauri runtime. Nothing here takes a
//! Tauri type, so it is testable against a temp-dir git remote.

use std::path::Path;

use chrono::Utc;

use crate::claude_local;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::job::JobTx;
use crate::paths::Paths;
use crate::registry::Repo;
use crate::setup;
use crate::state::RepoLink;

pub struct RepoProvision<'a> {
    pub repo: &'a Repo,
    pub worktree_path: &'a Path,
    pub branch: &'a str,
    pub paths: &'a Paths,
    pub tx: &'a JobTx,
}

/// Clone (if needed) → pull → resolve branch → worktree add → install
/// `.claude/settings.local.json` symlink → run setup script. Returns the
/// `RepoLink` to push into state. Atomic for its own repo: any failure past
/// `worktree add` tears down this repo's worktree (and its branch, if Tethys
/// created it) before bubbling. Sibling repos remain the caller's
/// responsibility (we don't know whether more still need provisioning).
pub async fn provision_repo_worktree(ctx: RepoProvision<'_>) -> AppResult<RepoLink> {
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

    let branch_preexisted = git::branch_exists(&clone_path, ctx.branch).await?;
    let remote_exists = !branch_preexisted
        && git::remote_branch_exists(&clone_path, "origin", ctx.branch).await?;
    let plan = git::plan_branch(ctx.branch, branch_preexisted, remote_exists);
    let created_branch = plan.created_branch;

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
            plan.source.as_ref(),
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

pub struct RepoTeardown<'a> {
    pub repo_key: &'a str,
    pub worktree_path: &'a Path,
    pub branch: &'a str,
    /// Whether Tethys created this branch. A pre-existing branch (e.g. a PR
    /// branch checked out for local edits) is left intact — only the worktree
    /// is removed.
    pub created_branch: bool,
    pub paths: &'a Paths,
    pub tx: &'a JobTx,
}

/// Best-effort reverse of `provision_repo_worktree`: force-remove the
/// worktree, prune stale registrations, and delete the branch when Tethys
/// created it. Errors are streamed as status events but never bubbled —
/// teardown is always best-effort.
pub async fn teardown_repo_worktree(ctx: RepoTeardown<'_>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::process::Command as StdCommand;

    fn git_ok(dir: &Path, args: &[&str]) {
        let out = StdCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_origin(dir: &Path) {
        let out = StdCommand::new("git")
            .args(["init", "-b", "main"])
            .arg(dir)
            .output()
            .expect("git init");
        assert!(out.status.success());
        git_ok(dir, &["config", "user.email", "t@example.com"]);
        git_ok(dir, &["config", "user.name", "T"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        git_ok(dir, &["add", "."]);
        git_ok(dir, &["commit", "-m", "init"]);
    }

    fn repo(key: &str, origin: &Path, setup_script: Option<&str>) -> Repo {
        Repo {
            key: key.into(),
            remote_url: origin.to_string_lossy().into_owned(),
            default_branch: None,
            default_setup_script: setup_script.map(String::from),
            setup_timeout_secs: Some(30),
            copy_files: Vec::new(),
            scripts: BTreeMap::new(),
            claude_notes: None,
            github_slug: None,
        }
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        origin: std::path::PathBuf,
        paths: Paths,
        worktree_root: std::path::PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        init_origin(&origin);
        let worktree_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&worktree_root).unwrap();
        Fixture {
            paths: Paths {
                data_dir: tmp.path().join("data"),
            },
            origin,
            worktree_root,
            _tmp: tmp,
        }
    }

    #[tokio::test]
    async fn provisions_a_worktree_on_a_fresh_branch() {
        let f = fixture();
        let wt = f.worktree_root.join("ws-1").join("api");
        let link = provision_repo_worktree(RepoProvision {
            repo: &repo("api", &f.origin, None),
            worktree_path: &wt,
            branch: "feat/new",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap();

        assert!(wt.join("README.md").exists());
        assert_eq!(git_out(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat/new");
        assert!(
            link.created_branch,
            "Tethys created this branch, so it owns it"
        );
    }

    /// Checking out a branch that already exists is how you pick up a PR
    /// branch for local edits — and Tethys must not claim ownership of it.
    #[tokio::test]
    async fn checking_out_an_existing_branch_does_not_claim_ownership() {
        let f = fixture();
        // Provision once to create the branch and the clone...
        let first = f.worktree_root.join("ws-1").join("api");
        provision_repo_worktree(RepoProvision {
            repo: &repo("api", &f.origin, None),
            worktree_path: &first,
            branch: "feat/shared",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap();
        // ...then release it so a second workspace can check it out.
        let clone = f.paths.repo_clone_path("api");
        git_ok(&clone, &["worktree", "remove", "--force", &first.to_string_lossy()]);

        let second = f.worktree_root.join("ws-2").join("api");
        let link = provision_repo_worktree(RepoProvision {
            repo: &repo("api", &f.origin, None),
            worktree_path: &second,
            branch: "feat/shared",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap();

        assert!(
            !link.created_branch,
            "the branch already existed — Purge must never delete it"
        );
    }

    /// The teardown contract, in the case that actually exercises it: a setup
    /// script fails after `worktree add` has already put things on disk.
    #[tokio::test]
    async fn a_failing_setup_script_removes_the_worktree_and_its_own_branch() {
        let f = fixture();
        let wt = f.worktree_root.join("ws-1").join("api");
        let err = provision_repo_worktree(RepoProvision {
            repo: &repo("api", &f.origin, Some("exit 7")),
            worktree_path: &wt,
            branch: "feat/doomed",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains('7'), "{err}");
        assert!(!wt.exists(), "worktree removed");
        let clone = f.paths.repo_clone_path("api");
        let branches = git_out(&clone, &["branch", "--list", "feat/doomed"]);
        assert!(
            branches.is_empty(),
            "Tethys created this branch, so rollback deletes it: {branches:?}"
        );
    }

    /// Same failure, but the branch pre-existed. The worktree goes; the user's
    /// branch stays. This is the invariant that decides whether a rollback can
    /// destroy work the user did outside Tethys.
    #[tokio::test]
    async fn a_failing_setup_script_leaves_a_pre_existing_branch_intact() {
        let f = fixture();
        // Create the branch via a first, successful provision, then release it.
        let first = f.worktree_root.join("ws-1").join("api");
        provision_repo_worktree(RepoProvision {
            repo: &repo("api", &f.origin, None),
            worktree_path: &first,
            branch: "feat/precious",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap();
        let clone = f.paths.repo_clone_path("api");
        git_ok(&clone, &["worktree", "remove", "--force", &first.to_string_lossy()]);

        let second = f.worktree_root.join("ws-2").join("api");
        let err = provision_repo_worktree(RepoProvision {
            repo: &repo("api", &f.origin, Some("exit 7")),
            worktree_path: &second,
            branch: "feat/precious",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains('7'), "{err}");
        assert!(!second.exists(), "worktree still removed");
        let branches = git_out(&clone, &["branch", "--list", "feat/precious"]);
        assert!(
            branches.contains("feat/precious"),
            "a branch Tethys did not create must survive rollback: {branches:?}"
        );
    }

    /// `copy_files` carries gitignored files (.env and friends) that
    /// `git worktree add` won't.
    #[tokio::test]
    async fn configured_files_are_copied_from_the_clone() {
        let f = fixture();
        let mut r = repo("api", &f.origin, None);
        r.copy_files = vec![".env".into()];

        // Seed the clone by provisioning once, then drop a file into it.
        let first = f.worktree_root.join("ws-0").join("api");
        provision_repo_worktree(RepoProvision {
            repo: &repo("api", &f.origin, None),
            worktree_path: &first,
            branch: "feat/a",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap();
        std::fs::write(f.paths.repo_clone_path("api").join(".env"), "SECRET=1").unwrap();

        let wt = f.worktree_root.join("ws-1").join("api");
        provision_repo_worktree(RepoProvision {
            repo: &r,
            worktree_path: &wt,
            branch: "feat/b",
            paths: &f.paths,
            tx: &JobTx::silent(),
        })
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "SECRET=1");
    }

    /// Paths that could escape the worktree are rejected before any copying.
    #[tokio::test]
    async fn copy_files_rejects_paths_that_escape_the_worktree() {
        let f = fixture();
        for bad in ["../outside", "/etc/passwd", "nested/../../escape"] {
            let mut r = repo("api", &f.origin, None);
            r.copy_files = vec![bad.into()];
            let wt = f.worktree_root.join("ws-x").join("api");
            let err = provision_repo_worktree(RepoProvision {
                repo: &r,
                worktree_path: &wt,
                branch: "feat/x",
                paths: &f.paths,
                tx: &JobTx::silent(),
            })
            .await
            .unwrap_err();
            assert!(err.to_string().contains("relative path"), "{bad}: {err}");
        }
    }
}
