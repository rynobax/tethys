use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("workspace not found: {0}")]
    WorkspaceNotFound(String),

    #[error("that would make the two workspaces wait on each other")]
    BlockerWouldCycle,

    #[error("a workspace can only wait on one in the same folder")]
    BlockerInAnotherFolder,

    #[error("folder not found: {0}")]
    FolderNotFound(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Other(err.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
