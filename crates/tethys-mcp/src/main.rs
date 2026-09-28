//! The MCP server each session spawns, forwarding tool calls to Tethys over
//! `mcp.sock`.
//!
//! stdout belongs to the protocol; diagnostics go to stderr. Unlike
//! `tethys-hook`, failures are loud: an agent that wrongly believes it handed
//! work off carries on as though the work is covered.

use std::borrow::Cow;
use std::env;
use std::path::PathBuf;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::io::stdio;
use rmcp::{ErrorData, RoleServer, ServiceExt};
use serde::Deserialize;
use serde_json::json;
use tokio::net::UnixStream;

use tethys_mcp::{
    read_frame, write_frame, CreateWorkspace, LinkPr, Request, Response, ENV_REPO_KEYS,
    ENV_SESSION_ID, ENV_SOCKET, ENV_WORKSPACE_ID, TOOL_CREATE_WORKSPACE, TOOL_LINK_PR,
};

#[derive(Debug, Deserialize)]
struct CreateWorkspaceArgs {
    repos: Vec<String>,
    branch: String,
    brief: String,
    #[serde(default)]
    blocks_caller: bool,
}

#[derive(Debug, Deserialize)]
struct LinkPrArgs {
    reference: String,
    #[serde(default)]
    repo_key: Option<String>,
}

#[derive(Debug, Clone)]
struct TethysServer {
    socket: PathBuf,
    from_workspace: String,
    from_session: Option<String>,
    repo_keys: Vec<String>,
}

impl TethysServer {
    fn from_env() -> anyhow::Result<Self> {
        let socket = env::var(ENV_SOCKET)
            .map_err(|_| anyhow::anyhow!("{ENV_SOCKET} is not set"))?;
        let from_workspace = env::var(ENV_WORKSPACE_ID)
            .map_err(|_| anyhow::anyhow!("{ENV_WORKSPACE_ID} is not set"))?;
        let repo_keys = env::var(ENV_REPO_KEYS)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        Ok(Self {
            socket: PathBuf::from(socket),
            from_workspace,
            from_session: env::var(ENV_SESSION_ID).ok().filter(|s| !s.is_empty()),
            repo_keys,
        })
    }

    /// Enumerating the registry means an unknown repo is refused now, not
    /// minutes later as a failed provision.
    fn create_workspace_schema(&self) -> JsonObject {
        let repo_items = if self.repo_keys.is_empty() {
            json!({ "type": "string" })
        } else {
            json!({ "type": "string", "enum": self.repo_keys })
        };
        let schema = json!({
            "type": "object",
            "properties": {
                "repos": {
                    "type": "array",
                    "items": repo_items,
                    "minItems": 1,
                    "description": "Repo keys the new workspace should span. \
                        Each becomes a git worktree checked out on `branch`.",
                },
                "branch": {
                    "type": "string",
                    "description": "Branch to create in every listed repo. If the \
                        name is already in use today's date is appended (then a \
                        number, if that's taken too), and the branch actually \
                        used is reported back.",
                },
                "brief": {
                    "type": "string",
                    "description": "The first message for the session that picks \
                        this work up. Write it for a fresh agent with no memory of \
                        this conversation: what to do, why, and anything it cannot \
                        discover from the code. This is the only thing carried \
                        across — you cannot follow up.",
                },
                "blocks_caller": {
                    "type": "boolean",
                    "description": "Set true when you cannot continue until this \
                        work lands. Marks the workspace you are in as waiting on \
                        the new one, which shows up in Tethys as a nested row. \
                        Purely a visual reminder — nothing is paused or gated, and \
                        you will not be told when it clears. Leave it out for work \
                        that runs alongside yours.",
                },
            },
            "required": ["repos", "branch", "brief"],
            "additionalProperties": false,
        });
        schema
            .as_object()
            .cloned()
            .expect("input schema literal is an object")
    }

    fn create_workspace_tool(&self) -> Tool {
        Tool::new(
            Cow::Borrowed(TOOL_CREATE_WORKSPACE),
            Cow::Borrowed(
                "Hand work off to a new Tethys workspace, running independently of \
                 this session. Creates a fresh git worktree per listed repo, all on \
                 a new branch, then starts one Claude session at the workspace root \
                 with your brief as its first message.\n\n\
                 Provisioning (clone, dependency install, setup scripts) takes \
                 minutes and happens in the background: this call returns as soon as \
                 the handoff is accepted, not when the workspace is ready. Tethys \
                 builds one workspace at a time, so a handoff asked for while others \
                 are still setting up waits its turn. You will \
                 not hear back from it, cannot read its progress, and cannot send it \
                 anything further — so put everything it needs in the brief.\n\n\
                 Reach for it when work should proceed on its own branch, in \
                 parallel with what you are doing. It is the wrong tool for work \
                 belonging on the current branch, which you should just do.",
            ),
            self.create_workspace_schema(),
        )
    }

    fn link_pr_schema(&self) -> JsonObject {
        let mut repo_key = json!({
            "type": "string",
            "description": "Which of this workspace's repos the PR belongs to. \
                Only needed when the workspace spans more than one GitHub repo \
                and you are passing a bare number.",
        });
        if !self.repo_keys.is_empty() {
            repo_key["enum"] = json!(self.repo_keys);
        }
        let schema = json!({
            "type": "object",
            "properties": {
                "reference": {
                    "type": "string",
                    "description": "The pull request: a full GitHub URL, \
                        `owner/repo#123`, or just the number.",
                },
                "repo_key": repo_key,
            },
            "required": ["reference"],
            "additionalProperties": false,
        });
        schema
            .as_object()
            .cloned()
            .expect("input schema literal is an object")
    }

    fn link_pr_tool(&self) -> Tool {
        Tool::new(
            Cow::Borrowed(TOOL_LINK_PR),
            Cow::Borrowed(
                "Show a pull request on the Tethys workspace this session belongs \
                 to, so Ryan sees its state — CI, reviews, conflicts — on the row \
                 for this work without going looking for it.\n\n\
                 Call it right after you open a PR. Tethys finds the PR for the \
                 workspace's own branch by itself, so the case this exists for is a \
                 PR you opened from some other branch in the same worktree — a \
                 stacked PR, a follow-up, a fix cut off main. Calling it for the \
                 branch PR anyway is harmless and simply makes it appear sooner.\n\n\
                 The PR must already exist on GitHub: this records it, it does not \
                 create or modify anything. Fails if the number is wrong or the \
                 repo isn't one this workspace spans.",
            ),
            self.link_pr_schema(),
        )
    }

    /// Protocol `2026-07-28` requires `ttlMs`, which `with_all_items` omits;
    /// without it Claude Code retries until "tools fetch failed". Zero, since
    /// the `repos` enum is fixed at spawn.
    fn tools_result(&self) -> ListToolsResult {
        ListToolsResult::with_all_items(vec![self.create_workspace_tool(), self.link_pr_tool()])
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private)
    }

    async fn send(&self, request: &Request) -> anyhow::Result<Response> {
        let mut stream = UnixStream::connect(&self.socket).await.map_err(|e| {
            anyhow::anyhow!(
                "could not reach Tethys at {}: {e}",
                self.socket.display()
            )
        })?;
        write_frame(&mut stream, request).await?;
        let response: Response = read_frame(&mut stream).await?;
        Ok(response)
    }
}

impl ServerHandler for TethysServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tethys", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Tethys manages parallel agent sessions across git worktrees. \
                 Use create_workspace to hand a distinct piece of work to a fresh \
                 workspace with its own branch and its own session.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(self.tools_result())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match request.name.as_ref() {
            TOOL_CREATE_WORKSPACE => self.create_workspace(request.arguments).await,
            TOOL_LINK_PR => self.link_pr(request.arguments).await,
            other => Err(ErrorData::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
    }
}

/// Failures past the argument parse are tool-level errors, which the agent
/// reads, rather than protocol errors, which the client may swallow.
impl TethysServer {
    async fn create_workspace(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args: CreateWorkspaceArgs = parse_args(arguments)?;
        let request = Request::CreateWorkspace(CreateWorkspace {
            from_workspace: self.from_workspace.clone(),
            from_session: self.from_session.clone(),
            repos: args.repos,
            branch: args.branch,
            brief: args.brief,
            blocks_caller: args.blocks_caller,
        });

        let response = match self.send(&request).await {
            Ok(response) => response,
            Err(e) => return Ok(failed(format!(
                "handoff failed, no workspace was created: {e}"
            ))),
        };

        Ok(match response {
            Response::Accepted {
                workspace_id,
                branch,
            } => CallToolResult::success(vec![ContentBlock::text(format!(
                "Handoff accepted. Workspace {workspace_id} is provisioning on branch \
                 `{branch}`; its session starts with your brief once the worktrees are \
                 ready. Nothing further is reported back here — if provisioning fails, \
                 Ryan sees it in Tethys."
            ))])
            .into(),
            Response::Rejected { message } => failed(format!(
                "handoff refused, no workspace was created: {message}"
            )),
            other => failed(format!("Tethys answered a handoff with {other:?}")),
        })
    }

    async fn link_pr(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args: LinkPrArgs = parse_args(arguments)?;
        let request = Request::LinkPr(LinkPr {
            from_workspace: self.from_workspace.clone(),
            from_session: self.from_session.clone(),
            repo_key: args.repo_key,
            reference: args.reference,
        });

        let response = match self.send(&request).await {
            Ok(response) => response,
            Err(e) => return Ok(failed(format!("link failed, nothing was linked: {e}"))),
        };

        Ok(match response {
            Response::Linked {
                repo_key,
                number,
                url,
                is_branch_pr,
            } => {
                let role = if is_branch_pr {
                    "this workspace's branch PR"
                } else {
                    "an extra PR on that repo"
                };
                CallToolResult::success(vec![ContentBlock::text(format!(
                    "Linked PR #{number} ({url}) to this workspace's {repo_key} repo, as \
                     {role}. Tethys polls it from here on, so its CI and review state \
                     show up on the workspace row."
                ))])
                .into()
            }
            Response::Rejected { message } => {
                failed(format!("link refused, nothing was linked: {message}"))
            }
            other => failed(format!("Tethys answered a link with {other:?}")),
        })
    }
}

fn parse_args<T: serde::de::DeserializeOwned>(
    arguments: Option<JsonObject>,
) -> Result<T, ErrorData> {
    serde_json::from_value(serde_json::Value::Object(arguments.unwrap_or_default()))
        .map_err(|e| ErrorData::invalid_params(format!("bad arguments: {e}"), None))
}

fn failed(message: String) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(message)]).into()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = TethysServer::from_env()?;
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> TethysServer {
        TethysServer {
            socket: PathBuf::from("/tmp/mcp.sock"),
            from_workspace: "ws-1".into(),
            from_session: Some("sess-1".into()),
            repo_keys: vec!["nl-frontend".into(), "nl-backend".into()],
        }
    }

    /// Nothing in the type system asks for `ttlMs`, so only this test holds it.
    #[test]
    fn the_tools_reply_carries_a_freshness_ttl() {
        let raw = serde_json::to_value(server().tools_result()).expect("serialize");
        assert_eq!(raw["ttlMs"], 0, "reply was {raw}");
        assert_eq!(raw["cacheScope"], "private");
    }

    #[test]
    fn the_repos_argument_enumerates_the_registry() {
        let schema = serde_json::to_value(server().create_workspace_schema()).expect("serialize");
        assert_eq!(
            schema["properties"]["repos"]["items"]["enum"],
            serde_json::json!(["nl-frontend", "nl-backend"])
        );
        assert_eq!(
            schema["required"],
            serde_json::json!(["repos", "branch", "brief"])
        );
    }

    #[test]
    fn link_pr_requires_only_the_reference() {
        let schema = serde_json::to_value(server().link_pr_schema()).expect("serialize");
        assert_eq!(schema["required"], serde_json::json!(["reference"]));
        assert_eq!(
            schema["properties"]["repo_key"]["enum"],
            serde_json::json!(["nl-frontend", "nl-backend"])
        );
    }

    /// An empty `enum` would match nothing.
    #[test]
    fn an_empty_registry_leaves_the_link_pr_enum_out() {
        let mut server = server();
        server.repo_keys.clear();
        let schema = serde_json::to_value(server.link_pr_schema()).expect("serialize");
        assert!(schema["properties"]["repo_key"]["enum"].is_null());
        assert_eq!(schema["properties"]["repo_key"]["type"], "string");
    }

    #[test]
    fn an_empty_registry_leaves_the_enum_out() {
        let mut server = server();
        server.repo_keys.clear();
        let schema = serde_json::to_value(server.create_workspace_schema()).expect("serialize");
        assert!(schema["properties"]["repos"]["items"]["enum"].is_null());
        assert_eq!(schema["properties"]["repos"]["items"]["type"], "string");
    }
}
