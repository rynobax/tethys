use std::io;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const SERVER_NAME: &str = "tethys";

pub const TOOL_CREATE_WORKSPACE: &str = "create_workspace";
pub const TOOL_LINK_PR: &str = "link_pr";
pub const TOOL_ADD_REPOS: &str = "add_repos";
pub const TOOL_REQUEST_MEMORY_BUDGET: &str = "request_memory_budget";
pub const TOOL_LEAVE_SHIPPING: &str = "leave_shipping";

/// Bare, as codex grants them.
pub const TOOL_NAMES: &[&str] = &[
    TOOL_CREATE_WORKSPACE,
    TOOL_LINK_PR,
    TOOL_ADD_REPOS,
    TOOL_REQUEST_MEMORY_BUDGET,
    TOOL_LEAVE_SHIPPING,
];

/// Qualified, as Claude's `--allowed-tools` wants them. An unlisted tool
/// stalls on a permission dialog nobody is watching.
pub const ALLOWED_TOOLS: &[&str] = &[
    "mcp__tethys__create_workspace",
    "mcp__tethys__link_pr",
    "mcp__tethys__add_repos",
    "mcp__tethys__request_memory_budget",
    "mcp__tethys__leave_shipping",
];

pub const ENV_SOCKET: &str = "TETHYS_MCP_SOCKET";
pub const ENV_WORKSPACE_ID: &str = "TETHYS_MCP_WORKSPACE_ID";
pub const ENV_SESSION_ID: &str = "TETHYS_MCP_SESSION_ID";
/// Comma-separated; becomes the `repos` enum in the tool schema.
pub const ENV_REPO_KEYS: &str = "TETHYS_MCP_REPO_KEYS";

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    CreateWorkspace(CreateWorkspace),
    LinkPr(LinkPr),
    AddRepos(AddRepos),
    RequestMemoryBudget(RequestMemoryBudget),
    LeaveShipping(LeaveShipping),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkspace {
    pub from_workspace: String,
    #[serde(default)]
    pub from_session: Option<String>,
    pub repos: Vec<String>,
    pub branch: String,
    pub brief: String,
    #[serde(default)]
    pub as_child: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkPr {
    /// Also the workspace the PR is linked to: an agent names the PR, never
    /// the workspace.
    pub from_workspace: String,
    #[serde(default)]
    pub from_session: Option<String>,
    /// `None` to infer from the reference or the workspace's only GitHub repo.
    #[serde(default)]
    pub repo_key: Option<String>,
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddRepos {
    /// Also the workspace the repos are added to, as with `LinkPr`.
    pub from_workspace: String,
    #[serde(default)]
    pub from_session: Option<String>,
    pub repos: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMemoryBudget {
    /// Also the session whose budget is raised.
    pub from_workspace: String,
    #[serde(default)]
    pub from_session: Option<String>,
    pub gb: u64,
    pub minutes: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaveShipping {
    /// Also the workspace taken out of Shipping, as with `LinkPr`.
    pub from_workspace: String,
    #[serde(default)]
    pub from_session: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddedRepo {
    pub repo_key: String,
    pub worktree_path: String,
    /// False when it was already in the workspace before this call.
    pub added: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Accepted {
        workspace_id: String,
        /// Differs from the one asked for when that name was taken.
        branch: String,
    },
    Linked {
        repo_key: String,
        number: u32,
        url: String,
        is_branch_pr: bool,
    },
    Added {
        branch: String,
        repos: Vec<AddedRepo>,
    },
    BudgetApproved {
        gb: u64,
        /// Local wall-clock time, `HH:MM`.
        until: String,
    },
    BudgetDenied,
    LeftShipping,
    Rejected {
        message: String,
    },
}

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {} bytes exceeds the cap", payload.len()),
        ));
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&payload).await?;
    writer.flush().await
}

pub async fn read_frame<R, T>(reader: &mut R) -> io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} out of bounds"),
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_without_as_child_makes_a_top_level_workspace() {
        let raw = r#"{
            "op": "create_workspace",
            "from_workspace": "ws-1",
            "repos": ["nl-backend"],
            "branch": "feat/handoff",
            "brief": "Do the thing."
        }"#;
        let Request::CreateWorkspace(req) = serde_json::from_str(raw).expect("must deserialize")
        else {
            panic!("must parse as a create_workspace request")
        };
        assert!(!req.as_child);
        assert_eq!(req.from_session, None);
    }

    #[tokio::test]
    async fn a_link_frame_round_trips_under_its_own_tag() {
        let req = Request::LinkPr(LinkPr {
            from_workspace: "ws-1".into(),
            from_session: Some("sess-1".into()),
            repo_key: Some("nl-backend".into()),
            reference: "https://github.com/me/api/pull/12".into(),
        });

        let raw = serde_json::to_value(&req).expect("serialize");
        assert_eq!(raw["op"], "link_pr");

        let mut buf = Vec::new();
        write_frame(&mut buf, &req).await.expect("write");
        let mut cursor = std::io::Cursor::new(buf);
        let back: Request = read_frame(&mut cursor).await.expect("read");
        let Request::LinkPr(back) = back else {
            panic!("must round-trip as a link_pr request")
        };
        assert_eq!(back.reference, "https://github.com/me/api/pull/12");
        assert_eq!(back.repo_key.as_deref(), Some("nl-backend"));
    }

    #[test]
    fn a_link_frame_without_a_repo_key_parses() {
        let raw = r##"{
            "op": "link_pr",
            "from_workspace": "ws-1",
            "reference": "#12"
        }"##;
        let Request::LinkPr(req) = serde_json::from_str(raw).expect("must deserialize") else {
            panic!("must parse as a link_pr request")
        };
        assert_eq!(req.repo_key, None);
        assert_eq!(req.from_session, None);
    }

    #[test]
    fn an_add_repos_frame_parses_under_its_own_tag() {
        let raw = r#"{
            "op": "add_repos",
            "from_workspace": "ws-1",
            "repos": ["nl-backend"]
        }"#;
        let Request::AddRepos(req) = serde_json::from_str(raw).expect("must deserialize") else {
            panic!("must parse as an add_repos request")
        };
        assert_eq!(req.repos, vec!["nl-backend".to_string()]);
        assert_eq!(req.from_session, None);
    }

    #[tokio::test]
    async fn a_zero_length_frame_is_rejected() {
        let mut cursor = std::io::Cursor::new(0u32.to_be_bytes().to_vec());
        let got: io::Result<Request> = read_frame(&mut cursor).await;
        assert_eq!(got.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
