//! The wire format shared by the `tethys-mcp` companion binary and the Tethys
//! app that answers its frames.
//!
//! Same discipline as [`tethys_hook`]: one type, defined once, so a field
//! can't be added to the sender and silently dropped by the receiver. The
//! failure contract is the opposite one, though. The hook must never disrupt a
//! Claude session, so it swallows everything; a handoff that silently didn't
//! happen is worse than an error, so everything here surfaces.

use std::io;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Name the server registers under, so its tools are addressed as
/// `mcp__tethys__<tool>` in Claude's permission strings.
pub const SERVER_NAME: &str = "tethys";

/// The one tool. Named for what it does to Tethys, not for the concept —
/// "handoff" is the word for the act, `create_workspace` is the effect the
/// calling agent asks for.
pub const TOOL_CREATE_WORKSPACE: &str = "create_workspace";

/// Fully-qualified tool name, as Claude's permission system spells it.
pub const ALLOWED_TOOL: &str = "mcp__tethys__create_workspace";

/// Env keys Tethys bakes into the generated `--mcp-config` at spawn time.
/// The calling session's identity arrives this way rather than as tool
/// arguments, so an agent cannot claim an origin that isn't its own.
pub const ENV_SOCKET: &str = "TETHYS_MCP_SOCKET";
pub const ENV_WORKSPACE_ID: &str = "TETHYS_MCP_WORKSPACE_ID";
pub const ENV_SESSION_ID: &str = "TETHYS_MCP_SESSION_ID";
/// Comma-separated registry repo keys, used to build the tool's `repos` enum
/// so a calling agent can't name a repo that doesn't exist. Fixed for the life
/// of the session — Tethys only reloads `repos.toml` at boot, so this is as
/// fresh as the app's own view of the registry.
pub const ENV_REPO_KEYS: &str = "TETHYS_MCP_REPO_KEYS";

/// Cap on a single frame in either direction. A brief is prose, so the only
/// way past this is a bug or a hostile caller.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// One request from the MCP server to the app. An enum with a single variant
/// today; the tag is what lets a second tool land without a second socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    CreateWorkspace(CreateWorkspace),
}

/// A handoff: everything the agent asked for, plus the identity Tethys baked
/// into the server's environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkspace {
    /// Workspace the calling session belongs to. Recorded as the new
    /// workspace's `Origin`, and the source of the `claude_binary` the new
    /// workspace inherits.
    pub from_workspace: String,
    /// Calling session's Tethys id. `None` only if the config predates the
    /// field or was hand-written.
    #[serde(default)]
    pub from_session: Option<String>,
    /// Registry repo keys to span. Constrained by the tool schema's enum, and
    /// re-checked against the registry on arrival.
    pub repos: Vec<String>,
    /// Branch to create in every repo. Sanitized and auto-suffixed if taken.
    pub branch: String,
    /// The Brief — first message for the session that gets the work.
    pub brief: String,
    /// When true, the calling workspace is marked as waiting on the new one.
    /// Default false so a config or client predating the field still parses.
    #[serde(default)]
    pub blocks_caller: bool,
}

/// The app's answer. A handoff is accepted or refused; there is deliberately
/// no third state, because the caller returns before provisioning starts and
/// never learns how it went.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Accepted {
        workspace_id: String,
        /// The branch actually used, which differs from the one asked for when
        /// that name was already taken.
        branch: String,
    },
    Rejected {
        message: String,
    },
}

/// Write a length-prefixed JSON frame.
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

/// Read one length-prefixed JSON frame.
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

    /// A config or client predating `blocks_caller` sends the frame without
    /// it. Rejecting that would break every handoff, not just blocking ones.
    #[test]
    fn a_frame_without_blocks_caller_parses_as_non_blocking() {
        let raw = r#"{
            "op": "create_workspace",
            "from_workspace": "ws-1",
            "repos": ["nl-backend"],
            "branch": "feat/handoff",
            "brief": "Do the thing."
        }"#;
        let Request::CreateWorkspace(req) = serde_json::from_str(raw).expect("must deserialize");
        assert!(!req.blocks_caller);
        assert_eq!(req.from_session, None);
    }

    #[tokio::test]
    async fn a_frame_round_trips() {
        let req = Request::CreateWorkspace(CreateWorkspace {
            from_workspace: "ws-1".into(),
            from_session: Some("sess-1".into()),
            repos: vec!["nl-backend".into()],
            branch: "feat/handoff".into(),
            brief: "Port the poller to the new seam.".into(),
            blocks_caller: true,
        });

        let mut buf = Vec::new();
        write_frame(&mut buf, &req).await.expect("write");

        let mut cursor = std::io::Cursor::new(buf);
        let back: Request = read_frame(&mut cursor).await.expect("read");
        let Request::CreateWorkspace(back) = back;
        assert_eq!(back.branch, "feat/handoff");
        assert_eq!(back.from_session.as_deref(), Some("sess-1"));
        assert_eq!(back.repos, vec!["nl-backend".to_string()]);
    }

    /// A truncated prefix must be an error, not a hang or a panic.
    #[tokio::test]
    async fn a_short_frame_is_an_error() {
        let mut cursor = std::io::Cursor::new(vec![0u8, 0, 1]);
        let got: io::Result<Request> = read_frame(&mut cursor).await;
        assert!(got.is_err());
    }

    #[tokio::test]
    async fn a_zero_length_frame_is_rejected() {
        let mut cursor = std::io::Cursor::new(0u32.to_be_bytes().to_vec());
        let got: io::Result<Request> = read_frame(&mut cursor).await;
        assert_eq!(got.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
