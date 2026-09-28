use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::job::{JobTx, LogStream};

const STDERR_TAIL_LINES: usize = 10;

/// Carries the stderr tail because jobs without a visible log pane (the
/// purger, a rollback) have nowhere else to surface why they failed.
pub struct RunOutcome {
    pub status: std::process::ExitStatus,
    stderr_tail: Vec<String>,
}

impl RunOutcome {
    pub fn success(&self) -> bool {
        self.status.success()
    }

    pub fn check(&self, op: impl std::fmt::Display) -> AppResult<()> {
        if self.status.success() {
            return Ok(());
        }
        Err(AppError::Other(self.failure_message(op)))
    }

    pub fn failure_message(&self, op: impl std::fmt::Display) -> String {
        let mut msg = format!("{op} exited with {:?}", self.status.code());
        if !self.stderr_tail.is_empty() {
            msg.push_str(": ");
            msg.push_str(&self.stderr_tail.join("; "));
        }
        msg
    }
}

pub async fn run_streamed<I, S>(
    program: &str,
    args: I,
    cwd: Option<&Path>,
    tx: &JobTx,
    repo: Option<&str>,
) -> AppResult<RunOutcome>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.env("GIT_TERMINAL_PROMPT", "0");

    let mut child = cmd.spawn().map_err(|e| {
        AppError::Other(format!("failed to spawn `{program}`: {e}"))
    })?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let tx_out = tx.clone();
    let repo_out = repo.map(String::from);
    let stdout_task = tokio::spawn(async move {
        drain_lines(stdout, &tx_out, LogStream::Stdout, repo_out.as_deref()).await
    });

    let tx_err = tx.clone();
    let repo_err = repo.map(String::from);
    let stderr_task = tokio::spawn(async move {
        drain_lines(stderr, &tx_err, LogStream::Stderr, repo_err.as_deref()).await
    });

    let status = child.wait().await?;
    let _ = stdout_task.await;
    let stderr_tail = stderr_task.await.unwrap_or_default();

    Ok(RunOutcome {
        status,
        stderr_tail,
    })
}

/// A clone interrupted before HEAD was written fails this.
async fn is_valid_clone(clone_path: &Path) -> bool {
    let result = Command::new("git")
        .arg("-C")
        .arg(clone_path)
        .arg("rev-parse")
        .arg("--verify")
        .arg("HEAD")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    matches!(result, Ok(s) if s.success())
}

/// Splits on `\r` too: progress output overwrites its line with a bare `\r`.
async fn drain_lines<R: AsyncRead + Unpin>(
    mut reader: R,
    tx: &JobTx,
    stream: LogStream,
    repo: Option<&str>,
) -> Vec<String> {
    let mut tail: std::collections::VecDeque<String> =
        std::collections::VecDeque::with_capacity(STDERR_TAIL_LINES);
    let mut push_tail = |line: &str| {
        if tail.len() == STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line.to_string());
    };
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::with_capacity(256);
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                for &byte in &buf[..n] {
                    if byte == b'\n' || byte == b'\r' {
                        if !line.is_empty() {
                            let text = String::from_utf8_lossy(&line).into_owned();
                            push_tail(&text);
                            tx.log(stream, text, repo);
                            line.clear();
                        }
                    } else {
                        line.push(byte);
                    }
                }
            }
            Err(_) => break,
        }
    }
    if !line.is_empty() {
        let text = String::from_utf8_lossy(&line).into_owned();
        push_tail(&text);
        tx.log(stream, text, repo);
    }
    tail.into()
}

pub async fn ensure_clone(
    clone_path: &Path,
    remote_url: &str,
    tx: &JobTx,
    repo: &str,
) -> AppResult<()> {
    if clone_path.exists() {
        if is_valid_clone(clone_path).await {
            tx.status(
                format!("clone already present at {}", clone_path.display()),
                Some(repo),
            );
            return Ok(());
        }
        tx.status(
            format!(
                "clone at {} is incomplete; removing and retrying",
                clone_path.display()
            ),
            Some(repo),
        );
        tokio::fs::remove_dir_all(clone_path).await.map_err(|e| {
            AppError::Other(format!(
                "failed to remove broken clone at {}: {e}",
                clone_path.display()
            ))
        })?;
    }

    tx.status(format!("cloning {remote_url}"), Some(repo));

    if let Some(parent) = clone_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    run_streamed(
        "git",
        [
            "clone".as_ref(),
            // Suppressed by default when stderr is a pipe.
            "--progress".as_ref(),
            remote_url.as_ref(),
            clone_path.as_os_str(),
        ],
        None,
        tx,
        Some(repo),
    )
    .await?
    .check(format!("git clone {remote_url}"))?;
    Ok(())
}

/// Tethys never touches the clone's checkout, so a failure means it's in a bad
/// state and branching off it would silently use stale code.
pub async fn pull_clone(clone_path: &Path, tx: &JobTx, repo: &str) -> AppResult<()> {
    tx.status("updating clone from origin".to_string(), Some(repo));
    let args: [&OsStr; 4] = [
        "-C".as_ref(),
        clone_path.as_os_str(),
        "pull".as_ref(),
        "--ff-only".as_ref(),
    ];
    run_streamed("git", args, None, tx, Some(repo))
        .await?
        .check(format!("git pull --ff-only in {}", clone_path.display()))?;
    Ok(())
}

pub enum WorktreeBranch<'a> {
    NewFromHead,
    /// e.g. `origin/<branch>`.
    TrackRemote(&'a str),
    /// Git refuses a branch checked out in another worktree, which is the
    /// guard against two workspaces sharing a branch.
    ExistingLocal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPlan {
    pub source: OwnedWorktreeBranch,
    /// Purge deletes only branches Tethys created.
    pub created_branch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedWorktreeBranch {
    NewFromHead,
    TrackRemote(String),
    ExistingLocal,
}

impl OwnedWorktreeBranch {
    pub fn as_ref(&self) -> WorktreeBranch<'_> {
        match self {
            OwnedWorktreeBranch::NewFromHead => WorktreeBranch::NewFromHead,
            OwnedWorktreeBranch::TrackRemote(s) => WorktreeBranch::TrackRemote(s),
            OwnedWorktreeBranch::ExistingLocal => WorktreeBranch::ExistingLocal,
        }
    }
}

pub fn plan_branch(branch: &str, local_exists: bool, remote_exists: bool) -> BranchPlan {
    match (local_exists, remote_exists) {
        (true, _) => BranchPlan {
            source: OwnedWorktreeBranch::ExistingLocal,
            created_branch: false,
        },
        (false, true) => BranchPlan {
            source: OwnedWorktreeBranch::TrackRemote(format!("origin/{branch}")),
            created_branch: true,
        },
        (false, false) => BranchPlan {
            source: OwnedWorktreeBranch::NewFromHead,
            created_branch: true,
        },
    }
}

pub async fn worktree_add(
    clone_path: &Path,
    worktree_path: &Path,
    branch: &str,
    source: WorktreeBranch<'_>,
    tx: &JobTx,
    repo: &str,
) -> AppResult<()> {
    tx.status(
        format!("creating worktree at {}", worktree_path.display()),
        Some(repo),
    );

    if let Some(parent) = worktree_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut args: Vec<&OsStr> = vec![
        "-C".as_ref(),
        clone_path.as_os_str(),
        "worktree".as_ref(),
        "add".as_ref(),
    ];
    match source {
        WorktreeBranch::NewFromHead => {
            args.push("-b".as_ref());
            args.push(branch.as_ref());
            args.push(worktree_path.as_os_str());
        }
        WorktreeBranch::TrackRemote(start_point) => {
            args.push("--track".as_ref());
            args.push("-b".as_ref());
            args.push(branch.as_ref());
            args.push(worktree_path.as_os_str());
            args.push(start_point.as_ref());
        }
        WorktreeBranch::ExistingLocal => {
            args.push(worktree_path.as_os_str());
            args.push(branch.as_ref());
        }
    }

    run_streamed("git", args, None, tx, Some(repo))
        .await?
        .check(format!("git worktree add {}", worktree_path.display()))?;
    Ok(())
}

/// `pull_clone` and `worktree_add` both build on whatever the clone has checked
/// out. With no override and no `origin/HEAD`, leave it alone rather than guess.
pub async fn ensure_clone_on_default_branch(
    clone_path: &Path,
    override_branch: Option<&str>,
    tx: &JobTx,
    repo: &str,
) -> AppResult<()> {
    let default = match override_branch {
        Some(branch) => branch.to_string(),
        None => {
            let Some(detected) = origin_default_branch(clone_path).await else {
                return Ok(());
            };
            detected
        }
    };
    if current_branch(clone_path).await.as_deref() == Some(default.as_str()) {
        return Ok(());
    }
    tx.status(
        format!("clone is not on '{default}'; switching back before branching"),
        Some(repo),
    );
    let args: [&OsStr; 4] = [
        "-C".as_ref(),
        clone_path.as_os_str(),
        "checkout".as_ref(),
        default.as_ref(),
    ];
    run_streamed("git", args, None, tx, Some(repo))
        .await?
        .check(format!(
            "git checkout {default} in {}",
            clone_path.display()
        ))?;
    Ok(())
}

async fn origin_default_branch(clone_path: &Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(clone_path)
        .arg("symbolic-ref")
        .arg("--short")
        .arg("refs/remotes/origin/HEAD")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    name.strip_prefix("origin/").map(str::to_string)
}

/// `None` when HEAD is detached.
async fn current_branch(clone_path: &Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(clone_path)
        .arg("symbolic-ref")
        .arg("--short")
        .arg("HEAD")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub async fn branch_exists(clone_path: &Path, branch: &str) -> AppResult<bool> {
    show_ref_exists(clone_path, &format!("refs/heads/{branch}")).await
}

/// Only as fresh as the clone's last pull.
pub async fn remote_branch_exists(
    clone_path: &Path,
    remote: &str,
    branch: &str,
) -> AppResult<bool> {
    show_ref_exists(clone_path, &format!("refs/remotes/{remote}/{branch}")).await
}

async fn show_ref_exists(clone_path: &Path, refspec: &str) -> AppResult<bool> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(clone_path)
        .arg("show-ref")
        .arg("--verify")
        .arg("--quiet")
        .arg(refspec)
        .output()
        .await
        .map_err(|e| AppError::Other(format!("git show-ref: {e}")))?;
    Ok(output.status.success())
}

/// Run before `branch -D`, which refuses a branch held by a prunable worktree.
pub async fn worktree_prune_best_effort(clone_path: &Path, tx: &JobTx, repo: &str) {
    let args: [&OsStr; 4] = [
        "-C".as_ref(),
        clone_path.as_os_str(),
        "worktree".as_ref(),
        "prune".as_ref(),
    ];
    match run_streamed("git", args, None, tx, Some(repo)).await {
        Ok(outcome) if outcome.success() => {}
        Ok(outcome) => tx.status(outcome.failure_message("worktree prune"), Some(repo)),
        Err(e) => tx.status(format!("worktree prune failed: {e}"), Some(repo)),
    }
}

pub async fn branch_delete_best_effort(
    clone_path: &Path,
    branch: &str,
    tx: &JobTx,
    repo: &str,
) {
    tx.status(format!("deleting branch {branch}"), Some(repo));
    let args: [&OsStr; 5] = [
        "-C".as_ref(),
        clone_path.as_os_str(),
        "branch".as_ref(),
        "-D".as_ref(),
        branch.as_ref(),
    ];
    match run_streamed("git", args, None, tx, Some(repo)).await {
        Ok(outcome) if outcome.success() => {}
        Ok(outcome) => tx.status(
            format!(
                "{} (already gone?)",
                outcome.failure_message(format!("branch -D {branch}"))
            ),
            Some(repo),
        ),
        Err(e) => tx.status(format!("branch -D {branch} failed: {e}"), Some(repo)),
    }
}

/// Errors on a dirty worktree unless `force`.
pub async fn worktree_remove(
    clone_path: &Path,
    worktree_path: &Path,
    force: bool,
    tx: &JobTx,
    repo: &str,
) -> AppResult<()> {
    tx.status(
        format!("removing worktree {}", worktree_path.display()),
        Some(repo),
    );

    let mut args: Vec<&OsStr> = vec![
        "-C".as_ref(),
        clone_path.as_os_str(),
        "worktree".as_ref(),
        "remove".as_ref(),
    ];
    if force {
        args.push("--force".as_ref());
    }
    args.push(worktree_path.as_os_str());

    run_streamed("git", args, None, tx, Some(repo))
        .await?
        .check(format!("git worktree remove {}", worktree_path.display()))?;
    Ok(())
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
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo_with_commit(dir: &Path) {
        let out = StdCommand::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(dir)
            .output()
            .expect("spawn git init");
        assert!(
            out.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        git_ok(dir, &["config", "user.email", "test@example.com"]);
        git_ok(dir, &["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        git_ok(dir, &["add", "."]);
        git_ok(dir, &["commit", "-m", "init"]);
    }

    fn clone(origin: &Path, dest: &Path) {
        let out = StdCommand::new("git")
            .arg("clone")
            .arg(origin)
            .arg(dest)
            .output()
            .expect("spawn git clone");
        assert!(
            out.status.success(),
            "git clone failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn origin_and_clone() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        init_repo_with_commit(&origin);
        let clone_path = tmp.path().join("clone");
        clone(&origin, &clone_path);
        (tmp, clone_path)
    }

    #[test]
    fn branch_planning_over_every_combination() {
        let local = plan_branch("feat/x", true, false);
        assert_eq!(local.source, OwnedWorktreeBranch::ExistingLocal);
        assert!(!local.created_branch, "never delete a branch we found");

        let both = plan_branch("feat/x", true, true);
        assert_eq!(both.source, OwnedWorktreeBranch::ExistingLocal);
        assert!(!both.created_branch);

        let remote = plan_branch("feat/x", false, true);
        assert_eq!(
            remote.source,
            OwnedWorktreeBranch::TrackRemote("origin/feat/x".into())
        );
        assert!(remote.created_branch);

        let fresh = plan_branch("feat/x", false, false);
        assert_eq!(fresh.source, OwnedWorktreeBranch::NewFromHead);
        assert!(fresh.created_branch);
    }

    fn recording_tx() -> (JobTx, tokio::sync::mpsc::UnboundedReceiver<crate::job::JobEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (JobTx(tx), rx)
    }

    fn logged_lines(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::job::JobEvent>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let crate::job::JobEvent::Log { line, .. } = ev {
                out.push(line);
            }
        }
        out
    }

    #[tokio::test]
    async fn splits_progress_output_on_carriage_returns() {
        let (tx, mut rx) = recording_tx();
        let outcome = run_streamed(
            "/bin/sh",
            ["-c", "printf 'a\rb\rc\n'"],
            None,
            &tx,
            None,
        )
        .await
        .unwrap();

        assert!(outcome.success());
        assert_eq!(logged_lines(&mut rx), vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn failure_carries_the_stderr_tail() {
        let (tx, _rx) = recording_tx();
        let outcome = run_streamed(
            "/bin/sh",
            ["-c", "echo 'fatal: not a git repository' >&2; exit 128"],
            None,
            &tx,
            None,
        )
        .await
        .unwrap();

        let err = outcome.check("git status").unwrap_err().to_string();
        assert!(err.contains("git status"), "{err}");
        assert!(err.contains("128"), "{err}");
        assert!(err.contains("fatal: not a git repository"), "{err}");
    }

    #[tokio::test]
    async fn stderr_tail_is_bounded() {
        let (tx, _rx) = recording_tx();
        let outcome = run_streamed(
            "/bin/sh",
            ["-c", "for i in $(seq 1 50); do echo line$i >&2; done; exit 1"],
            None,
            &tx,
            None,
        )
        .await
        .unwrap();

        let msg = outcome.failure_message("op");
        assert!(msg.contains("line50"), "keeps the last line: {msg}");
        assert!(!msg.contains("line40"), "drops older lines: {msg}");
    }

    #[tokio::test]
    async fn silent_sink_discards_output() {
        let outcome = run_streamed(
            "/bin/sh",
            ["-c", "echo hello; echo oops >&2; exit 3"],
            None,
            &JobTx::silent(),
            None,
        )
        .await
        .unwrap();

        assert!(!outcome.success());
        assert!(outcome.failure_message("op").contains("oops"));
    }

    #[tokio::test]
    async fn switches_clone_back_to_default_branch() {
        let (_tmp, clone_path) = origin_and_clone();
        git_ok(&clone_path, &["checkout", "-b", "stray"]);
        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("stray"));

        ensure_clone_on_default_branch(&clone_path, None, &JobTx::silent(), "repo")
            .await
            .unwrap();

        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn switches_clone_to_overridden_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        init_repo_with_commit(&origin);
        git_ok(&origin, &["checkout", "-b", "develop"]);
        git_ok(&origin, &["checkout", "main"]);

        let clone_path = tmp.path().join("clone");
        clone(&origin, &clone_path);
        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("main"));

        ensure_clone_on_default_branch(&clone_path, Some("develop"), &JobTx::silent(), "repo")
            .await
            .unwrap();

        assert_eq!(
            current_branch(&clone_path).await.as_deref(),
            Some("develop")
        );
    }

    #[tokio::test]
    async fn leaves_clone_on_default_branch_untouched() {
        let (_tmp, clone_path) = origin_and_clone();
        assert_eq!(
            origin_default_branch(&clone_path).await.as_deref(),
            Some("main")
        );
        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("main"));

        ensure_clone_on_default_branch(&clone_path, None, &JobTx::silent(), "repo")
            .await
            .unwrap();

        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn worktree_add_checks_out_existing_local_branch() {
        let (tmp, clone_path) = origin_and_clone();
        git_ok(&clone_path, &["branch", "feature"]);
        assert!(branch_exists(&clone_path, "feature").await.unwrap());

        let worktree_path = tmp.path().join("wt");
        worktree_add(
            &clone_path,
            &worktree_path,
            "feature",
            WorktreeBranch::ExistingLocal,
            &JobTx::silent(),
            "repo",
        )
        .await
        .unwrap();

        assert_eq!(
            current_branch(&worktree_path).await.as_deref(),
            Some("feature")
        );
    }

    #[tokio::test]
    async fn worktree_add_existing_local_rejects_branch_in_use() {
        let (tmp, clone_path) = origin_and_clone();
        git_ok(&clone_path, &["branch", "feature"]);

        let first = tmp.path().join("wt1");
        worktree_add(
            &clone_path,
            &first,
            "feature",
            WorktreeBranch::ExistingLocal,
            &JobTx::silent(),
            "repo",
        )
        .await
        .unwrap();

        let second = tmp.path().join("wt2");
        let result = worktree_add(
            &clone_path,
            &second,
            "feature",
            WorktreeBranch::ExistingLocal,
            &JobTx::silent(),
            "repo",
        )
        .await;
        assert!(result.is_err());
    }
}
