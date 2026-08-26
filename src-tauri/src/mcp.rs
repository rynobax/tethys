//! Both halves of the seam in front of the Tethys MCP server.
//!
//! [`McpLaunch`] is the spawn side: it renders the `--mcp-config` a Claude
//! session is launched with. [`listen`] is the receiving side: the socket that
//! config points at.
//!
//! The two are here together because they are one contract read from opposite
//! ends — the config names a binary, a socket and an identity, and the listener
//! is what answers on the other end of it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use crate::error::AppResult;
use crate::github;
use crate::handoff::Handoff;
use crate::paths::Paths;
use crate::registry::RegistryLoad;
use crate::store::Store;

pub use tethys_mcp::{CreateWorkspace, LinkPr, Request, Response};

/// Everything needed to render a session's `--mcp-config`, resolved once at
/// boot: the companion binary, the socket it should dial, and the registry repo
/// keys that become the tool's `repos` enum.
#[derive(Debug, Clone)]
pub struct McpLaunch {
    server_bin: PathBuf,
    socket: PathBuf,
    repo_keys: Vec<String>,
}

impl McpLaunch {
    /// `None` when the companion binary isn't sitting next to the app. A
    /// session then spawns without the handoff tool, which is a session that
    /// works — so this is a warning, not a startup failure.
    pub fn resolve(paths: &Paths, registry: &RegistryLoad) -> Option<Self> {
        let server_bin = match crate::paths::tethys_mcp_bin() {
            Ok(p) if p.exists() => p,
            Ok(p) => {
                warn!(
                    path = %p.display(),
                    "tethys-mcp binary not found — sessions can't hand work off"
                );
                return None;
            }
            Err(e) => {
                warn!(error = %e, "could not resolve tethys-mcp path");
                return None;
            }
        };
        // An empty registry leaves the `repos` enum out of the schema, which is
        // the honest rendering of "there is nothing to pick from".
        let repo_keys = registry
            .require()
            .map(|reg| reg.repos.iter().map(|r| r.key.clone()).collect())
            .unwrap_or_default();
        Some(Self {
            server_bin,
            socket: paths.mcp_socket(),
            repo_keys,
        })
    }

    /// The `claude` flags that put the Tethys tools in a session's hands.
    ///
    /// `--mcp-config` takes JSON inline, so nothing is written to disk. The
    /// calling identity rides in the server's `env` block rather than being
    /// passed as a tool argument, which is what makes an `Origin` trustworthy:
    /// the agent never gets to say who it is.
    ///
    /// Deliberately no `--strict-mcp-config` — that would cut the session off
    /// from every other MCP server the user has configured.
    ///
    /// Both flags are spelled `--flag=value` rather than `--flag value`. Both
    /// are variadic in `claude --help` (`<configs...>`, `<tools...>`), and a
    /// variadic flag eats every following argument that isn't itself a flag —
    /// which would silently swallow the Brief, since that's a trailing
    /// positional. The `=` form takes exactly one value and stops.
    pub fn claude_args(&self, workspace_id: &str, session_id: &str) -> Vec<String> {
        vec![
            format!("--mcp-config={}", self.config_json(workspace_id, session_id)),
            format!("--allowed-tools={}", tethys_mcp::ALLOWED_TOOLS.join(",")),
        ]
    }

    fn config_json(&self, workspace_id: &str, session_id: &str) -> String {
        json!({
            "mcpServers": {
                tethys_mcp::SERVER_NAME: {
                    "command": self.server_bin,
                    "env": {
                        tethys_mcp::ENV_SOCKET: self.socket,
                        tethys_mcp::ENV_WORKSPACE_ID: workspace_id,
                        tethys_mcp::ENV_SESSION_ID: session_id,
                        tethys_mcp::ENV_REPO_KEYS: self.repo_keys.join(","),
                    },
                },
            },
        })
        .to_string()
    }
}

/// What the socket can reach — one field per thing an agent is allowed to ask
/// Tethys for. Bundled rather than passed loose so a third tool costs a field
/// here and nothing at the call site.
#[derive(Clone)]
pub struct McpServices {
    pub handoff: Arc<Handoff>,
    pub store: Arc<Store>,
    pub registry: Arc<RegistryLoad>,
}

/// Bind `mcp.sock` and spawn an accept loop. If the socket already exists (a
/// prior run died without cleanup) it's removed first.
pub async fn listen(socket_path: &Path, services: McpServices) -> AppResult<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path).await.ok();
    }
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let listener = UnixListener::bind(socket_path)?;
    info!(path = %socket_path.display(), "mcp socket listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let services = services.clone();
                    tokio::spawn(async move {
                        if let Err(e) = serve_connection(stream, services).await {
                            warn!(error = %e, "mcp connection error");
                        }
                    });
                }
                Err(e) => error!(error = %e, "mcp accept failed"),
            }
        }
    });

    Ok(())
}

/// One request, one reply, then the peer hangs up.
///
/// A rejection is a reply, not a dropped connection: the calling agent has to
/// be told, in words, that nothing happened.
async fn serve_connection(mut stream: UnixStream, services: McpServices) -> AppResult<()> {
    let request: Request = tethys_mcp::read_frame(&mut stream).await?;
    let response = match request {
        Request::CreateWorkspace(req) => create_workspace(&services, req).await,
        Request::LinkPr(req) => link_pr(&services, req).await,
    };
    tethys_mcp::write_frame(&mut stream, &response).await?;
    Ok(())
}

async fn create_workspace(services: &McpServices, req: CreateWorkspace) -> Response {
    debug!(
        from_workspace = %req.from_workspace,
        branch = %req.branch,
        repos = req.repos.len(),
        "handoff requested"
    );
    match services.handoff.accept(req).await {
        Ok(accepted) => Response::Accepted {
            workspace_id: accepted.workspace_id,
            branch: accepted.branch,
        },
        Err(e) => {
            warn!(error = %e, "handoff refused");
            Response::Rejected {
                message: e.to_string(),
            }
        }
    }
}

/// The same door the attach dialog goes through — the agent only supplies the
/// reference, and the workspace comes off the connection's baked-in identity.
async fn link_pr(services: &McpServices, req: LinkPr) -> Response {
    debug!(
        from_workspace = %req.from_workspace,
        from_session = ?req.from_session,
        reference = %req.reference,
        "pr link requested"
    );
    let attached = github::attach(
        &services.store,
        &services.registry,
        &req.from_workspace,
        req.repo_key.as_deref(),
        &req.reference,
    )
    .await;
    match attached {
        Ok(attached) => Response::Linked {
            repo_key: attached.repo_key,
            number: attached.status.pr_number,
            url: attached.status.url,
            is_branch_pr: attached.is_branch_pr,
        },
        Err(e) => {
            warn!(error = %e, "pr link refused");
            Response::Rejected {
                message: e.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> McpLaunch {
        McpLaunch {
            server_bin: PathBuf::from("/opt/tethys/tethys-mcp"),
            socket: PathBuf::from("/tmp/app/mcp.sock"),
            repo_keys: vec!["nl-frontend".into(), "nl-backend".into()],
        }
    }

    /// The identity has to be inside the config, not in the tool schema — an
    /// agent that could pass its own `from_workspace` could forge an origin.
    #[test]
    fn the_config_carries_the_calling_identity() {
        let raw = launch().config_json("ws-1", "sess-1");
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
        let env = &parsed["mcpServers"]["tethys"]["env"];
        assert_eq!(env[tethys_mcp::ENV_WORKSPACE_ID], "ws-1");
        assert_eq!(env[tethys_mcp::ENV_SESSION_ID], "sess-1");
        assert_eq!(env[tethys_mcp::ENV_REPO_KEYS], "nl-frontend,nl-backend");
        assert_eq!(
            parsed["mcpServers"]["tethys"]["command"],
            "/opt/tethys/tethys-mcp"
        );
    }

    /// `--strict-mcp-config` would disable every other MCP server the user has
    /// configured, so it must never appear here.
    #[test]
    fn the_args_never_go_strict() {
        let args = launch().claude_args("ws-1", "sess-1");
        assert!(!args.iter().any(|a| a.starts_with("--strict-mcp-config")));
    }

    /// Both flags are variadic, so a bare `--flag value` pair would let them
    /// swallow the Brief that follows on the command line. Every arg has to
    /// carry its own value.
    #[test]
    fn every_arg_carries_its_own_value() {
        let args = launch().claude_args("ws-1", "sess-1");
        assert_eq!(args.len(), 2);
        for arg in &args {
            assert!(arg.starts_with("--") && arg.contains('='), "{arg}");
        }
        assert!(args[0].starts_with("--mcp-config={"));
        assert_eq!(
            args[1],
            "--allowed-tools=mcp__tethys__create_workspace,mcp__tethys__link_pr"
        );
    }
}
