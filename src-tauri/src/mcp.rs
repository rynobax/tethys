use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use crate::agent_cmd::toml_string;
use crate::budget::{Budgets, Decision, OverrideRequest};
use crate::error::{AppError, AppResult};
use crate::github;
use crate::handoff::Handoff;
use crate::job::JobTx;
use crate::paths::Paths;
use crate::provision::{self, RepoAddition};
use crate::provision_queue::ProvisionQueue;
use crate::registry::RegistryLoad;
use crate::store::Store;

pub use tethys_mcp::{
    AddRepos, AddedRepo, CreateWorkspace, LeaveShipping, LinkPr, Request, RequestMemoryBudget,
    Response,
};

const TOOL_TIMEOUT_SECS: u32 = 60 * 60;

#[derive(Debug, Clone)]
pub struct McpLaunch {
    server_bin: PathBuf,
    socket: PathBuf,
    repo_keys: Vec<String>,
}

impl McpLaunch {
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

    /// Identity rides in the server's `env`, never a tool argument, so an agent
    /// can't forge its `Origin`. No `--strict-mcp-config`: it would cut the
    /// session off from the user's other MCP servers.
    ///
    /// `--flag=value` because both flags are variadic and `--flag value` would
    /// swallow the trailing Brief.
    pub fn claude_args(&self, workspace_id: &str, session_id: &str) -> Vec<String> {
        vec![
            format!("--mcp-config={}", self.config_json(workspace_id, session_id)),
            format!("--allowed-tools={}", tethys_mcp::ALLOWED_TOOLS.join(",")),
        ]
    }

    /// `approval_mode = "approve"` is codex's `--allowed-tools`, scoped to this
    /// server's tools, so no call stalls on a dialog nobody is watching.
    pub fn codex_args(&self, workspace_id: &str, session_id: &str) -> Vec<String> {
        let server = tethys_mcp::SERVER_NAME;
        let mut args = vec![
            "-c".into(),
            format!(
                "mcp_servers.{server}.command={}",
                toml_string(&self.server_bin.to_string_lossy())
            ),
            "-c".into(),
            format!(
                "mcp_servers.{server}.env={{{}}}",
                [
                    (tethys_mcp::ENV_SOCKET, self.socket.to_string_lossy().into_owned()),
                    (tethys_mcp::ENV_WORKSPACE_ID, workspace_id.to_string()),
                    (tethys_mcp::ENV_SESSION_ID, session_id.to_string()),
                    (tethys_mcp::ENV_REPO_KEYS, self.repo_keys.join(",")),
                ]
                .iter()
                .map(|(k, v)| format!("{k}={}", toml_string(v)))
                .collect::<Vec<_>>()
                .join(",")
            ),
            // `add_repos` waits out clones, installs and the setup queue;
            // codex's 60s default would abandon it partway.
            "-c".into(),
            format!("mcp_servers.{server}.tool_timeout_sec={TOOL_TIMEOUT_SECS}"),
        ];
        for tool in tethys_mcp::TOOL_NAMES {
            args.push("-c".into());
            args.push(format!(
                "mcp_servers.{server}.tools.{tool}.approval_mode=\"approve\""
            ));
        }
        args
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

#[derive(Clone)]
pub struct McpServices {
    pub handoff: Arc<Handoff>,
    pub store: Arc<Store>,
    pub registry: Arc<RegistryLoad>,
    pub paths: Paths,
    pub queue: ProvisionQueue,
    pub budgets: Arc<Budgets>,
}

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

/// A rejection is a reply, not a dropped connection, so the agent is told in
/// words that nothing happened.
async fn serve_connection(mut stream: UnixStream, services: McpServices) -> AppResult<()> {
    let request: Request = tethys_mcp::read_frame(&mut stream).await?;
    let response = match request {
        Request::CreateWorkspace(req) => create_workspace(&services, req).await,
        Request::LinkPr(req) => link_pr(&services, req).await,
        Request::AddRepos(req) => add_repos(&services, req).await,
        Request::RequestMemoryBudget(req) => request_memory_budget(&services, req).await,
        Request::LeaveShipping(req) => leave_shipping(&services, req).await,
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

async fn add_repos(services: &McpServices, req: AddRepos) -> Response {
    debug!(
        from_workspace = %req.from_workspace,
        from_session = ?req.from_session,
        repos = ?req.repos,
        "repo add requested"
    );
    match add_missing_repos(services, &req).await {
        Ok(response) => response,
        Err(e) => {
            warn!(error = %e, "repo add refused");
            Response::Rejected {
                message: e.to_string(),
            }
        }
    }
}

async fn request_memory_budget(services: &McpServices, req: RequestMemoryBudget) -> Response {
    let Some(session_id) = req.from_session else {
        return Response::Rejected {
            message: "this session predates budget requests; restart it to use them".into(),
        };
    };
    let request = OverrideRequest {
        workspace_id: req.from_workspace,
        session_id,
        gb: req.gb,
        minutes: req.minutes,
        reason: req.reason,
    };
    let gb = request.gb;
    match services.budgets.request(request).await {
        Ok(Decision::Approved { until }) => Response::BudgetApproved {
            gb,
            until: until.format("%H:%M").to_string(),
        },
        Ok(Decision::Denied) => Response::BudgetDenied,
        Err(e) => Response::Rejected {
            message: e.to_string(),
        },
    }
}

async fn leave_shipping(services: &McpServices, req: LeaveShipping) -> Response {
    let left = services
        .store
        .update_workspace(&req.from_workspace, |ws| {
            if !ws.is_shipping() {
                return Err(AppError::Other("this workspace isn't Shipping".into()));
            }
            ws.shipment = None;
            Ok(())
        })
        .await;
    match left {
        Ok(()) => {
            info!(workspace = %req.from_workspace, reason = %req.reason, "agent took workspace out of shipping");
            Response::LeftShipping
        }
        Err(e) => Response::Rejected {
            message: e.to_string(),
        },
    }
}

/// A repo already linked is reported back rather than refused, so an agent
/// retrying an interrupted call gets its paths instead of an error.
async fn add_missing_repos(services: &McpServices, req: &AddRepos) -> AppResult<Response> {
    let reg = services.registry.require()?;
    let workspace_id = &req.from_workspace;

    let mut wanted: Vec<String> = Vec::with_capacity(req.repos.len());
    for key in &req.repos {
        if !wanted.contains(key) {
            wanted.push(key.clone());
        }
    }
    let missing: Vec<String> = services
        .store
        .with_workspace(workspace_id, |w| {
            wanted.iter().filter(|k| !w.has_link(k)).cloned().collect()
        })
        .await?;

    let workspace = if missing.is_empty() {
        services.store.with_workspace(workspace_id, |w| w.clone()).await?
    } else {
        let added = provision::add_repos(RepoAddition {
            workspace_id,
            repo_keys: &missing,
            registry: reg,
            paths: &services.paths,
            store: &services.store,
            queue: &services.queue,
            tx: &JobTx::silent(),
        })
        .await;
        match added {
            Ok(ws) => ws,
            Err(e) => return Err(with_partial_progress(services, workspace_id, &missing, e).await),
        }
    };

    let repos = wanted
        .iter()
        .filter_map(|key| workspace.repo_links.iter().find(|l| &l.repo_key == key))
        .map(|link| AddedRepo {
            repo_key: link.repo_key.clone(),
            worktree_path: link.worktree_path.display().to_string(),
            added: missing.contains(&link.repo_key),
        })
        .collect();
    Ok(Response::Added {
        branch: workspace.branch,
        repos,
    })
}

/// Repos before a failure stay added, and the agent should know which.
async fn with_partial_progress(
    services: &McpServices,
    workspace_id: &str,
    missing: &[String],
    error: AppError,
) -> AppError {
    let landed: Vec<String> = services
        .store
        .with_workspace(workspace_id, |w| {
            missing.iter().filter(|k| w.has_link(k)).cloned().collect()
        })
        .await
        .unwrap_or_default();
    if landed.is_empty() {
        return error;
    }
    AppError::Other(format!(
        "{error}. Added before the failure, and still in the workspace: {}",
        landed.join(", ")
    ))
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
            "--allowed-tools=mcp__tethys__create_workspace,mcp__tethys__link_pr,\
             mcp__tethys__add_repos,mcp__tethys__request_memory_budget,\
             mcp__tethys__leave_shipping"
        );
    }
}
