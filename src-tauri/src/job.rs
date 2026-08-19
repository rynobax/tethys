use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

/// Structured progress events emitted by long-running jobs (workspace create,
/// delete, etc.) over the per-job `tauri::ipc::Channel<JobEvent>`.
///
/// Defined as one type across all job kinds so the frontend log-pane component
/// is generic.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobEvent {
    /// Human-readable status headline, e.g. "cloning nl-frontend".
    Status {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    /// A single line of output from a child process.
    Log {
        stream: LogStream,
        line: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    /// Terminal: job completed successfully.
    Success,
    /// Terminal: job failed with the given message.
    Failed { error: String },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Shorthand wrapper around `UnboundedSender<JobEvent>` so call sites don't
/// have to keep typing the generics.
#[derive(Clone)]
pub struct JobTx(pub UnboundedSender<JobEvent>);

impl JobTx {
    /// A sink whose events go nowhere. For background jobs with no UI channel
    /// (the purger, a create rollback) that still want the same git functions
    /// the foreground path uses. Sends already discard their error, so a
    /// dropped receiver is the designed behaviour, not a special case.
    pub fn silent() -> Self {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        JobTx(tx)
    }

    pub fn status(&self, message: impl Into<String>, repo: Option<&str>) {
        let _ = self.0.send(JobEvent::Status {
            message: message.into(),
            repo: repo.map(String::from),
        });
    }

    pub fn log(&self, stream: LogStream, line: String, repo: Option<&str>) {
        let _ = self.0.send(JobEvent::Log {
            stream,
            line,
            repo: repo.map(String::from),
        });
    }
}
