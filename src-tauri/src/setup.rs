use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{AppError, AppResult};
use crate::job::{JobTx, LogStream};
use crate::repo_env::EnvVars;

const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Which of a repo's scripts is running, for the log and error lines.
#[derive(Clone, Copy)]
pub enum ScriptKind {
    Setup,
    Teardown,
}

impl ScriptKind {
    fn label(self) -> &'static str {
        match self {
            ScriptKind::Setup => "setup",
            ScriptKind::Teardown => "teardown",
        }
    }
}

pub async fn run_repo_script(
    kind: ScriptKind,
    script: &str,
    cwd: &Path,
    env: &EnvVars,
    timeout_secs: Option<u64>,
    tx: &JobTx,
    repo: &str,
) -> AppResult<()> {
    let label = kind.label();
    tx.status(format!("running {label}: {script}"), Some(repo));

    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(script);
    cmd.current_dir(cwd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::child_env::sanitize_for_child_repo(&mut cmd);
    cmd.envs(env.iter().map(|(k, v)| (k, v)));

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Other(format!("failed to spawn {label} script: {e}")))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let tx_out = tx.clone();
    let repo_out = repo.to_string();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tx_out.log(LogStream::Stdout, line, &repo_out);
        }
    });

    let tx_err = tx.clone();
    let repo_err = repo.to_string();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tx_err.log(LogStream::Stderr, line, &repo_err);
        }
    });

    let duration = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

    let wait_result = timeout(duration, child.wait()).await;

    let status = match wait_result {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(AppError::Io(e)),
        Err(_) => {
            tx.status(
                format!("{label} script timed out after {}s; terminating", duration.as_secs()),
                Some(repo),
            );
            let _ = child.start_kill();
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = child.kill().await;
            return Err(AppError::Other(format!(
                "{label} script timed out after {}s",
                duration.as_secs()
            )));
        }
    };

    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if !status.success() {
        return Err(AppError::Other(format!(
            "{label} script exited with {:?}",
            status.code()
        )));
    }
    Ok(())
}
