use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::job::{JobTx, LogStream};

/// Run a child process, streaming each line of stdout/stderr as `JobEvent::Log`
/// via the provided `JobTx`. Blocks until the child exits. Returns the exit
/// status so the caller decides what to do on non-zero.
///
/// `repo` is attached to each emitted event so the UI can group output by repo.
pub async fn run_streamed<I, S>(
    program: &str,
    args: I,
    cwd: Option<&Path>,
    tx: &JobTx,
    repo: Option<&str>,
) -> AppResult<std::process::ExitStatus>
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
    cmd.env("GIT_TERMINAL_PROMPT", "0"); // fail fast instead of hanging on auth prompt

    let mut child = cmd.spawn().map_err(|e| {
        AppError::Other(format!("failed to spawn `{program}`: {e}"))
    })?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let tx_out = tx.clone();
    let repo_out = repo.map(String::from);
    let stdout_task = tokio::spawn(async move {
        drain_lines(stdout, &tx_out, LogStream::Stdout, repo_out.as_deref()).await;
    });

    let tx_err = tx.clone();
    let repo_err = repo.map(String::from);
    let stderr_task = tokio::spawn(async move {
        drain_lines(stderr, &tx_err, LogStream::Stderr, repo_err.as_deref()).await;
    });

    let status = child.wait().await?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    Ok(status)
}

/// Probe whether `clone_path` looks like a complete git clone by asking
/// `git rev-parse HEAD`. A half-finished clone (process killed after `.git/`
/// was created but before HEAD was written) fails this check.
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

/// Read from `reader`, split on both `\n` and `\r` (git/yarn/pnpm progress
/// overwrites the current line with `\r` alone), and emit each segment as
/// a `JobEvent::Log`. Without splitting on `\r`, progress lines never
/// surface — the user just sees "Cloning into..." and then nothing for
/// minutes while the clone runs.
async fn drain_lines<R: AsyncRead + Unpin>(
    mut reader: R,
    tx: &JobTx,
    stream: LogStream,
    repo: Option<&str>,
) {
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::with_capacity(256);
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break, // EOF
            Ok(n) => {
                for &byte in &buf[..n] {
                    if byte == b'\n' || byte == b'\r' {
                        if !line.is_empty() {
                            tx.log(
                                stream,
                                String::from_utf8_lossy(&line).into_owned(),
                                repo,
                            );
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
        tx.log(stream, String::from_utf8_lossy(&line).into_owned(), repo);
    }
}

/// Clone `remote_url` into `clone_path` if it's not already a valid clone.
/// Partial/broken clones (e.g. from a previous run that was interrupted
/// mid-fetch) are detected via a `git rev-parse HEAD` probe and wiped so
/// the re-clone can succeed — otherwise `git clone` refuses to write into
/// a non-empty directory.
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

    let status = run_streamed(
        "git",
        [
            "clone".as_ref(),
            // Force progress output even when stderr is a pipe (default is
            // to suppress). Without this users see only "Cloning into..."
            // and then nothing for the duration of a multi-minute clone.
            "--progress".as_ref(),
            remote_url.as_ref(),
            clone_path.as_os_str(),
        ],
        None,
        tx,
        Some(repo),
    )
    .await?;

    if !status.success() {
        return Err(AppError::Other(format!(
            "git clone {remote_url} exited with {:?}",
            status.code()
        )));
    }
    Ok(())
}

/// `git -C <clone_path> pull --ff-only`. Tethys never modifies the clone's
/// working tree or checked-out branch, so a fast-forward pull should always
/// succeed when online. A failure means the clone is in a bad state (dirty
/// working tree, diverged history) and branching off it would silently use
/// stale code — bubble the error so workspace creation aborts loudly.
pub async fn pull_clone(clone_path: &Path, tx: &JobTx, repo: &str) -> AppResult<()> {
    tx.status("updating clone from origin".to_string(), Some(repo));
    let args: [&OsStr; 4] = [
        "-C".as_ref(),
        clone_path.as_os_str(),
        "pull".as_ref(),
        "--ff-only".as_ref(),
    ];
    let status = run_streamed("git", args, None, tx, Some(repo)).await?;
    if !status.success() {
        return Err(AppError::Other(format!(
            "git pull --ff-only in {} exited with {:?}",
            clone_path.display(),
            status.code()
        )));
    }
    Ok(())
}

/// Creates a new branch `<branch>` and a worktree checking it out.
///
/// With `track_from = None`: `git worktree add <worktree_path> -b <branch>` —
/// new branch starts at the clone's current HEAD with no upstream.
///
/// With `track_from = Some("origin/<branch>")`:
/// `git worktree add --track -b <branch> <worktree_path> origin/<branch>` —
/// new branch starts at the remote ref and is set to track it. Used when the
/// caller has already verified the remote branch exists, so the worktree
/// lands on the remote's commit with upstream wired up in one step.
/// How `worktree_add` should resolve the branch it checks out.
pub enum WorktreeBranch<'a> {
    /// Create a fresh branch off the clone's current HEAD (`-b <branch>`).
    NewFromHead,
    /// Create a fresh local branch tracking the given start point, e.g.
    /// `origin/<branch>` (`--track -b <branch> <path> <start>`).
    TrackRemote(&'a str),
    /// Check out a branch that already exists locally (`<path> <branch>`).
    /// Git refuses if that branch is already checked out in another worktree,
    /// which is the guard against two workspaces sharing a branch.
    ExistingLocal,
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

    let status = run_streamed("git", args, None, tx, Some(repo)).await?;

    if !status.success() {
        return Err(AppError::Other(format!(
            "git worktree add {} exited with {:?}",
            worktree_path.display(),
            status.code()
        )));
    }
    Ok(())
}

/// Ensure the clone is checked out on its default branch before we pull and
/// branch new worktrees off its HEAD.
///
/// `override_branch` is the repo's `default_branch` from `repos.toml` when the
/// user pinned one; otherwise we fall back to origin's default branch as
/// recorded in `refs/remotes/origin/HEAD`.
///
/// Tethys treats the clone as a stable base that always sits on the default
/// branch: `pull_clone` fast-forwards whatever is checked out, and
/// `worktree_add` with `track_from = None` branches off the clone's current
/// HEAD. If a stray manual checkout (or an interrupted git operation) left the
/// clone on some other branch, both of those silently use the wrong base, so
/// we detect that here and switch back.
///
/// Fallback detection is best-effort: with no override and a missing
/// `refs/remotes/origin/HEAD` we can't know the default branch and leave the
/// clone alone rather than guess. But once we know which branch we want, a
/// failed `checkout` (e.g. a dirty working tree, or a pinned branch that
/// doesn't exist) is bubbled so provisioning aborts loudly instead of branching
/// off stale code.
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
    let status = run_streamed("git", args, None, tx, Some(repo)).await?;
    if !status.success() {
        return Err(AppError::Other(format!(
            "git checkout {default} in {} exited with {:?}",
            clone_path.display(),
            status.code()
        )));
    }
    Ok(())
}

/// Origin's default branch as recorded in the clone's `refs/remotes/origin/HEAD`
/// (written by `git clone`), e.g. `"main"`. `None` if the ref is missing or
/// unreadable.
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
    // `--short` still yields "origin/main"; strip the remote prefix.
    name.strip_prefix("origin/").map(str::to_string)
}

/// The branch currently checked out in the clone, e.g. `"main"`. `None` if
/// unreadable or HEAD is detached.
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

/// `git -C <clone_path> show-ref --verify --quiet refs/heads/<branch>`.
/// Returns true if the branch exists locally in the clone. Non-zero exit
/// means the branch doesn't exist — not an error.
pub async fn branch_exists(clone_path: &Path, branch: &str) -> AppResult<bool> {
    show_ref_exists(clone_path, &format!("refs/heads/{branch}")).await
}

/// `git -C <clone_path> show-ref --verify --quiet refs/remotes/<remote>/<branch>`.
/// Returns true if the remote-tracking branch exists in the clone. The clone
/// is expected to be freshly pulled before this is called, so a `true` here
/// means the branch is genuinely present on the remote.
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

/// `git -C <clone_path> worktree prune`. Best-effort: clears stale worktree
/// registrations for directories that no longer exist. Errors are logged
/// but not bubbled. Run before `branch -D` so git won't refuse with "branch
/// in use by prunable worktree".
pub async fn worktree_prune_best_effort(clone_path: &Path, tx: &JobTx, repo: &str) {
    let args: [&OsStr; 4] = [
        "-C".as_ref(),
        clone_path.as_os_str(),
        "worktree".as_ref(),
        "prune".as_ref(),
    ];
    match run_streamed("git", args, None, tx, Some(repo)).await {
        Ok(status) if status.success() => {}
        Ok(status) => tx.status(
            format!("worktree prune exited with {:?}", status.code()),
            Some(repo),
        ),
        Err(e) => tx.status(format!("worktree prune failed: {e}"), Some(repo)),
    }
}

/// `git -C <clone_path> branch -D <branch>`. Best-effort: a non-zero exit
/// (e.g. the branch doesn't exist) is logged but not bubbled. Used as
/// cleanup when a workspace is deleted, so the same branch name can be
/// reused for a new workspace.
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
        Ok(status) if status.success() => {}
        Ok(status) => tx.status(
            format!("branch -D {branch} exited with {:?} (already gone?)", status.code()),
            Some(repo),
        ),
        Err(e) => tx.status(
            format!("branch -D {branch} failed: {e}"),
            Some(repo),
        ),
    }
}

/// `git -C <clone_path> worktree remove <worktree_path>`. Silent variant
/// for the background purger: no `JobTx`, no per-line streaming.
pub async fn worktree_remove_silent(
    clone_path: &Path,
    worktree_path: &Path,
    force: bool,
) -> AppResult<()> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(clone_path)
        .arg("worktree")
        .arg("remove");
    if force {
        cmd.arg("--force");
    }
    cmd.arg(worktree_path);
    let output = cmd
        .output()
        .await
        .map_err(|e| AppError::Other(format!("git worktree remove: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AppError::Other(format!(
            "git worktree remove {} exited with {:?}: {stderr}",
            worktree_path.display(),
            output.status.code()
        )));
    }
    Ok(())
}

/// `git -C <clone_path> worktree prune`. Silent best-effort variant.
pub async fn worktree_prune_best_effort_silent(clone_path: &Path) {
    let _ = tokio::process::Command::new("git")
        .arg("-C")
        .arg(clone_path)
        .arg("worktree")
        .arg("prune")
        .output()
        .await;
}

/// `git -C <clone_path> branch -D <branch>`. Silent best-effort variant.
pub async fn branch_delete_best_effort_silent(clone_path: &Path, branch: &str) {
    let _ = tokio::process::Command::new("git")
        .arg("-C")
        .arg(clone_path)
        .arg("branch")
        .arg("-D")
        .arg(branch)
        .output()
        .await;
}

/// `git -C <clone_path> worktree remove <worktree_path>`. Returns an error if
/// the worktree is dirty (caller can retry with `force`).
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

    let status = run_streamed("git", args, None, tx, Some(repo)).await?;

    if !status.success() {
        return Err(AppError::Other(format!(
            "git worktree remove {} exited with {:?}",
            worktree_path.display(),
            status.code()
        )));
    }
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

    /// Initialize `dir` as a git repo on `main` with a single commit.
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

    fn noop_tx() -> JobTx {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        JobTx(tx)
    }

    #[tokio::test]
    async fn switches_clone_back_to_default_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        init_repo_with_commit(&origin);

        let clone_path = tmp.path().join("clone");
        clone(&origin, &clone_path);

        // Simulate something accidentally checking out the wrong branch.
        git_ok(&clone_path, &["checkout", "-b", "stray"]);
        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("stray"));

        ensure_clone_on_default_branch(&clone_path, None, &noop_tx(), "repo")
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
        // A second branch on the origin the user wants to base worktrees off.
        git_ok(&origin, &["checkout", "-b", "develop"]);
        git_ok(&origin, &["checkout", "main"]);

        let clone_path = tmp.path().join("clone");
        clone(&origin, &clone_path);
        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("main"));

        ensure_clone_on_default_branch(&clone_path, Some("develop"), &noop_tx(), "repo")
            .await
            .unwrap();

        assert_eq!(
            current_branch(&clone_path).await.as_deref(),
            Some("develop")
        );
    }

    #[tokio::test]
    async fn leaves_clone_on_default_branch_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        init_repo_with_commit(&origin);

        let clone_path = tmp.path().join("clone");
        clone(&origin, &clone_path);
        assert_eq!(
            origin_default_branch(&clone_path).await.as_deref(),
            Some("main")
        );
        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("main"));

        ensure_clone_on_default_branch(&clone_path, None, &noop_tx(), "repo")
            .await
            .unwrap();

        assert_eq!(current_branch(&clone_path).await.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn worktree_add_checks_out_existing_local_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        init_repo_with_commit(&origin);

        let clone_path = tmp.path().join("clone");
        clone(&origin, &clone_path);
        // A branch that already exists locally — e.g. a PR branch the user
        // fetched and now wants to edit in a worktree.
        git_ok(&clone_path, &["branch", "feature"]);
        assert!(branch_exists(&clone_path, "feature").await.unwrap());

        let worktree_path = tmp.path().join("wt");
        worktree_add(
            &clone_path,
            &worktree_path,
            "feature",
            WorktreeBranch::ExistingLocal,
            &noop_tx(),
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
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        init_repo_with_commit(&origin);

        let clone_path = tmp.path().join("clone");
        clone(&origin, &clone_path);
        git_ok(&clone_path, &["branch", "feature"]);

        let first = tmp.path().join("wt1");
        worktree_add(
            &clone_path,
            &first,
            "feature",
            WorktreeBranch::ExistingLocal,
            &noop_tx(),
            "repo",
        )
        .await
        .unwrap();

        // A second worktree on the same branch is the "another workspace already
        // uses this branch" case — git must refuse it.
        let second = tmp.path().join("wt2");
        let result = worktree_add(
            &clone_path,
            &second,
            "feature",
            WorktreeBranch::ExistingLocal,
            &noop_tx(),
            "repo",
        )
        .await;
        assert!(result.is_err());
    }
}
