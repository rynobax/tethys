use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use tracing::{info, warn};

use crate::claude_local;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::inprogress::InProgressWorkspaces;
use crate::job::{JobEvent, JobTx};
use crate::paths::Paths;
use crate::provision_queue::ProvisionQueue;
use crate::reconcile;
use crate::registry::{Repo, RepoRegistry};
use crate::setup;
use crate::state::{RepoLink, Workspace, WorkspaceStatus};
use crate::store::Store;
use crate::workspace_doc;

pub struct RepoProvision<'a> {
    pub repo: &'a Repo,
    pub worktree_path: &'a Path,
    pub branch: &'a str,
    pub paths: &'a Paths,
    pub tx: &'a JobTx,
}

/// Atomic for its own repo only: sibling repos are the caller's to roll back.
pub async fn provision_repo_worktree(ctx: RepoProvision<'_>) -> AppResult<RepoLink> {
    let clone_path = ctx.paths.repo_clone_path(&ctx.repo.key);

    git::ensure_clone(&clone_path, &ctx.repo.remote_url, ctx.tx, &ctx.repo.key).await?;
    // A stray checkout would become the base of the pull and of any worktree
    // branched off HEAD.
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
            prs: Vec::new(),
            dismissed: Vec::new(),
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

/// Carries gitignored files (`.env` etc.) that `git worktree add` doesn't.
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
    /// A pre-existing branch (e.g. someone's PR) is never deleted.
    pub created_branch: bool,
    pub paths: &'a Paths,
    pub tx: &'a JobTx,
}

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

pub struct WorkspaceProvision<'a> {
    pub workspace_id: &'a str,
    pub branch: &'a str,
    pub workspace_dir: &'a str,
    pub repos: &'a [Repo],
    pub registry: &'a RepoRegistry,
    pub paths: &'a Paths,
    pub store: &'a Arc<Store>,
    pub in_progress: &'a InProgressWorkspaces,
    pub queue: &'a ProvisionQueue,
    pub tx: &'a JobTx,
}

pub async fn provision_workspace(ctx: WorkspaceProvision<'_>) -> AppResult<Workspace> {
    // Keeps the reconciler off our dirs. Taken before queueing so a handoff
    // landing meanwhile can't claim the same directory name.
    let _in_progress_guard = ctx.in_progress.insert(ctx.workspace_dir.to_string());

    let mut waited = false;
    let _slot = match ctx.queue.try_acquire() {
        Some(slot) => slot,
        None => {
            waited = true;
            ctx.tx.status(ctx.queue.wait_message(), None);
            // A handoff has no log pane, so the row has to say it.
            set_status(ctx.store, ctx.workspace_id, WorkspaceStatus::Queued).await;
            ctx.queue.acquire().await
        }
    };

    // Left `Queued`, not `CreationFailed`: it was called off, not failed.
    let live = ctx
        .store
        .with_workspace(ctx.workspace_id, |w| w.deleted_at.is_none())
        .await
        .unwrap_or(false);
    if !live {
        let msg = "workspace was deleted while it waited in the setup queue";
        info!(workspace = %ctx.workspace_id, "{msg}");
        let _ = ctx.tx.0.send(JobEvent::Failed { error: msg.into() });
        return Err(AppError::Other(msg.into()));
    }

    if waited {
        set_status(ctx.store, ctx.workspace_id, WorkspaceStatus::Creating).await;
    }

    let mut created: Vec<RepoLink> = Vec::new();
    let orchestrate = async {
        for repo in ctx.repos {
            let worktree_path = ctx.registry.plan_worktree_path(ctx.workspace_dir, &repo.key);
            let link = provision_repo_worktree(RepoProvision {
                repo,
                worktree_path: &worktree_path,
                branch: ctx.branch,
                paths: ctx.paths,
                tx: ctx.tx,
            })
            .await?;
            created.push(link);
        }
        Ok::<_, AppError>(())
    }
    .await;

    match orchestrate {
        Ok(()) => {
            let stored = ctx
                .store
                .update_workspace(ctx.workspace_id, |ws| {
                    ws.repo_links = created.clone();
                    ws.status = WorkspaceStatus::Ready;
                    Ok(ws.clone())
                })
                .await?;

            seed_workspace_root(&stored, ctx.registry, ctx.paths, ctx.tx).await;

            info!(
                id = %stored.id,
                branch = %stored.branch,
                repos = stored.repo_links.len(),
                "created workspace"
            );
            let _ = ctx.tx.0.send(JobEvent::Success);
            Ok(stored)
        }
        Err(e) => {
            let msg = e.to_string();
            warn!(error = %msg, "workspace create failed; rolling back worktrees");
            ctx.tx
                .status(format!("tearing down partial workspace: {msg}"), None);

            for link in created.iter().rev() {
                teardown_repo_worktree(RepoTeardown {
                    repo_key: &link.repo_key,
                    worktree_path: &link.worktree_path,
                    branch: ctx.branch,
                    created_branch: link.created_branch,
                    paths: ctx.paths,
                    tx: ctx.tx,
                })
                .await;
            }

            let parent = ctx.registry.worktree_root.join(ctx.workspace_dir);
            if parent.exists() && reconcile::is_under(&ctx.registry.worktree_root, &parent) {
                if let Err(e) = tokio::fs::remove_dir_all(&parent).await {
                    warn!(
                        path = %parent.display(),
                        error = %e,
                        "failed to remove partial workspace dir"
                    );
                }
            }

            let mutate_result = ctx
                .store
                .update_workspace(ctx.workspace_id, |ws| {
                    ws.status = WorkspaceStatus::CreationFailed {
                        error: msg.clone(),
                    };
                    Ok(())
                })
                .await;
            if let Err(mutate_err) = mutate_result {
                warn!(error = %mutate_err, "failed to mark workspace as CreationFailed");
            }

            let _ = ctx.tx.0.send(JobEvent::Failed { error: msg });
            Err(e)
        }
    }
}

async fn set_status(store: &Arc<Store>, workspace_id: &str, status: WorkspaceStatus) {
    if let Err(e) = store
        .update_workspace(workspace_id, |ws| {
            ws.status = status;
            Ok(())
        })
        .await
    {
        warn!(workspace = %workspace_id, error = %e, "could not update draft status");
    }
}

async fn seed_workspace_root(
    workspace: &Workspace,
    registry: &RepoRegistry,
    paths: &Paths,
    tx: &JobTx,
) {
    let Some(workspace_root) = workspace.root_buf() else {
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
        tx.status(format!("workspace-root settings seed failed: {e}"), None);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
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

    struct TestCtx {
        store: Arc<Store>,
        registry: RepoRegistry,
        queue: crate::provision_queue::ProvisionQueue,
        in_progress: InProgressWorkspaces,
    }

    impl TestCtx {
        async fn new(f: &Fixture, setup_script: &str) -> Self {
            let store = Store::load(
                f.paths.data_dir.join("state.json"),
                f.paths.data_dir.join("state.json.tmp"),
                Box::new(crate::store::NullNotifier),
            )
            .await
            .unwrap();
            store
                .mutate(|s| {
                    for id in ["ws-a", "ws-b"] {
                        s.workspaces.push(Workspace::draft(
                            id.into(),
                            format!("feat/{}", &id[3..]),
                            crate::agent::Agent::Claude,
                            None,
                            crate::state::Origin::Ui,
                            None,
                        ));
                    }
                    Ok(())
                })
                .await
                .unwrap();

            Self {
                store,
                registry: RepoRegistry {
                    worktree_root: f.worktree_root.clone(),
                    repos: vec![repo("api", &f.origin, Some(setup_script))],
                    workspace_doc: None,
                },
                queue: crate::provision_queue::ProvisionQueue::new(),
                in_progress: InProgressWorkspaces::new(),
            }
        }

        async fn provision(&self, f: &Fixture, id: &str, branch: &str) -> AppResult<Workspace> {
            provision_workspace(WorkspaceProvision {
                workspace_id: id,
                branch,
                workspace_dir: &crate::registry::sanitize_branch_for_dir(branch),
                repos: &self.registry.repos,
                registry: &self.registry,
                paths: &f.paths,
                store: &self.store,
                in_progress: &self.in_progress,
                queue: &self.queue,
                tx: &JobTx::silent(),
            })
            .await
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

    #[tokio::test]
    async fn checking_out_an_existing_branch_does_not_claim_ownership() {
        let f = fixture();
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

    #[tokio::test]
    async fn a_failing_setup_script_leaves_a_pre_existing_branch_intact() {
        let f = fixture();
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

    #[tokio::test]
    async fn configured_files_are_copied_from_the_clone() {
        let f = fixture();
        let mut r = repo("api", &f.origin, None);
        r.copy_files = vec![".env".into()];

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

    #[tokio::test]
    async fn two_workspaces_are_provisioned_one_after_the_other() {
        let f = fixture();
        let log = f.worktree_root.parent().unwrap().join("setup.log");
        let script = format!(
            "printf 'start\n' >> {log}; sleep 0.3; printf 'end\n' >> {log}",
            log = log.display()
        );
        let ctx = TestCtx::new(&f, &script).await;

        let (a, b) = tokio::join!(
            ctx.provision(&f, "ws-a", "feat/a"),
            ctx.provision(&f, "ws-b", "feat/b"),
        );
        a.unwrap();
        b.unwrap();

        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        assert_eq!(
            lines,
            vec!["start", "end", "start", "end"],
            "setup scripts overlapped"
        );
    }

    #[tokio::test]
    async fn a_workspace_deleted_while_queued_is_never_built() {
        let f = fixture();
        let ctx = TestCtx::new(&f, "sleep 0.4").await;

        let cancel = async {
            // "ws-b" is parked behind "ws-a" by now.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            ctx.store
                .update_workspace("ws-b", |ws| {
                    ws.deleted_at = Some(Utc::now());
                    Ok(())
                })
                .await
                .unwrap();
        };

        let (a, b, ()) = tokio::join!(
            ctx.provision(&f, "ws-a", "feat/a"),
            ctx.provision(&f, "ws-b", "feat/b"),
            cancel,
        );

        a.unwrap();
        let err = b.unwrap_err().to_string();
        assert!(err.contains("deleted"), "{err}");
        assert!(
            !f.worktree_root.join("feat-b").exists(),
            "an abandoned workspace leaves nothing on disk"
        );
        let status = ctx
            .store
            .with_workspace("ws-b", |w| w.status.clone())
            .await
            .unwrap();
        assert!(
            matches!(status, WorkspaceStatus::Queued),
            "left as it was — a deleted row has no failure to report: {status:?}"
        );
    }

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
