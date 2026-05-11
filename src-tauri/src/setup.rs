use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{AppError, AppResult};
use crate::job::{JobTx, LogStream};

const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Run a repo's setup script in the newly-created worktree.
/// Shell is `/bin/sh -c <script>` so users can pass full commands
/// (`"yarn install && yarn build"`).
///
/// `timeout_secs` defaults to `DEFAULT_TIMEOUT_SECS` (10 min) when `None`.
/// On timeout: SIGTERM, then SIGKILL after a 5s grace.
pub async fn run_setup_script(
    script: &str,
    cwd: &Path,
    timeout_secs: Option<u64>,
    tx: &JobTx,
    repo: &str,
) -> AppResult<()> {
    tx.status(format!("running setup: {script}"), Some(repo));

    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(script);
    cmd.current_dir(cwd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Other(format!("failed to spawn setup script: {e}")))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let tx_out = tx.clone();
    let repo_out = repo.to_string();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tx_out.log(LogStream::Stdout, line, Some(&repo_out));
        }
    });

    let tx_err = tx.clone();
    let repo_err = repo.to_string();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tx_err.log(LogStream::Stderr, line, Some(&repo_err));
        }
    });

    let duration = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

    let wait_result = timeout(duration, child.wait()).await;

    let status = match wait_result {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(AppError::Io(e)),
        Err(_) => {
            // Timed out — SIGTERM, then SIGKILL after grace.
            tx.status(
                format!("setup script timed out after {}s; terminating", duration.as_secs()),
                Some(repo),
            );
            let _ = child.start_kill();
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = child.kill().await;
            return Err(AppError::Other(format!(
                "setup script timed out after {}s",
                duration.as_secs()
            )));
        }
    };

    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if !status.success() {
        return Err(AppError::Other(format!(
            "setup script exited with {:?}",
            status.code()
        )));
    }
    Ok(())
}
