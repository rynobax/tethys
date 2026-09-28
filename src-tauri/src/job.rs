use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobEvent {
    Status {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    Log {
        stream: LogStream,
        line: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    Success,
    Failed { error: String },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Clone)]
pub struct JobTx(pub UnboundedSender<JobEvent>);

impl JobTx {
    /// For background jobs with no UI channel. Sends ignore errors, so the
    /// dropped receiver is harmless.
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
