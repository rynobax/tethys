//! MCP server companion binary — the tools a session gets over Tethys itself.
//!
//! Claude Code spawns one of these per session, from the `--mcp-config` Tethys
//! renders at spawn time. It exposes two tools — `create_workspace` to hand
//! work off, and `link_pr` to point the current workspace at a pull request —
//! and forwards each call to the running Tethys app over `mcp.sock`.
//!
//! Two things about this process are worth knowing:
//!
//! 1. **stdout belongs to the protocol.** Nothing may print there. Diagnostics
//!    go to stderr, where Claude collects them.
//! 2. **Failures are loud.** Its sibling `tethys-hook` exits 0 no matter what,
//!    because a broken hook must never disturb a session. Here the opposite
//!    holds: an agent that believes it handed work off when it didn't will
//!    carry on as though the work is covered.

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

/// What the calling agent supplies. Everything else — who is calling, which
/// repos exist — comes from the environment Tethys baked in.
#[derive(Debug, Deserialize)]
struct CreateWorkspaceArgs {
    repos: Vec<String>,
    branch: String,
    brief: String,
    #[serde(default)]
    blocks_caller: bool,
}

/// What the calling agent supplies to `link_pr`. The workspace it lands on
/// comes from the environment, so an agent can only ever link to its own.
#[derive(Debug, Deserialize)]
struct LinkPrArgs {
    reference: String,
    #[serde(default)]
    repo_key: Option<String>,
}

/// The server: a socket to talk to, an identity to stamp on requests, and the
/// repo keys that make up the `repos` enum.
#[derive(Debug, Clone)]
struct TethysServer {
    socket: PathBuf,
    from_workspace: String,
    from_session: Option<String>,
    repo_keys: Vec<String>,
}

impl TethysServer {
    /// Read the config Tethys baked into our environment. A missing socket or
    /// workspace id means we were launched by something other than Tethys, and
    /// there is nothing useful we could do.
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

    /// `create_workspace`'s input schema, built at runtime so `repos` can
    /// enumerate the registry. An enum means a calling agent cannot name a repo Tethys has
    /// never heard of — the failure it would otherwise learn about minutes
    /// later, from a workspace that failed to provision.
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
                        name is already in use a numeric suffix is added, and the \
                        branch actually used is reported back.",
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
                 the handoff is accepted, not when the workspace is ready. You will \
                 not hear back from it, cannot read its progress, and cannot send it \
                 anything further — so put everything it needs in the brief.\n\n\
                 Reach for it when work should proceed on its own branch, in \
                 parallel with what you are doing. It is the wrong tool for work \
                 belonging on the current branch, which you should just do.",
            ),
            self.create_workspace_schema(),
        )
    }

    /// `link_pr`'s input schema. `repo_key` enumerates the registry for the
    /// same reason `repos` does above, but it stays optional: naming a repo is
    /// only ever disambiguation, and a workspace with one GitHub repo — or a
    /// pasted URL that names its own — needs none.
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

    /// The `tools/list` reply.
    ///
    /// `ttl_ms` and `cache_scope` are not optional in practice. Claude Code
    /// negotiates protocol `2026-07-28`, which requires `ttlMs` on a paginated
    /// result (SEP-2549), and rmcp's `with_all_items` leaves it out — a reply
    /// without it is rejected and retried until the client gives up with
    /// "tools fetch failed". A ttl of 0 is the honest value: the `repos` enum
    /// is baked in when Tethys spawns this process, so a cached list must not
    /// outlive the session it was built for.
    fn tools_result(&self) -> ListToolsResult {
        ListToolsResult::with_all_items(vec![self.create_workspace_tool(), self.link_pr_tool()])
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private)
    }

    /// One request, one connection, one reply. Short-lived like the hook's, but
    /// this one waits for an answer.
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
                "Tethys manages parallel Claude sessions across git worktrees. \
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

/// The two calls. Both follow the same shape: parse the agent's arguments,
/// stamp the identity from the environment onto them, and turn whatever comes
/// back into words.
///
/// Everything past the parse is a *tool-level* error rather than a protocol
/// one. A protocol error can be swallowed by the client; the agent has to read
/// the reason it didn't get what it asked for, or it will assume it did.
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

/// Deserialize a tool call's arguments. The one place a bad call is a protocol
/// error: the agent sent something the schema said it couldn't.
fn parse_args<T: serde::de::DeserializeOwned>(
    arguments: Option<JsonObject>,
) -> Result<T, ErrorData> {
    serde_json::from_value(serde_json::Value::Object(arguments.unwrap_or_default()))
        .map_err(|e| ErrorData::invalid_params(format!("bad arguments: {e}"), None))
}

/// A tool-level failure: an error the agent reads, not one the client eats.
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

    /// Regression: without `ttlMs`, Claude Code rejects the reply outright and
    /// the tool never appears — it reports "tools fetch failed" after retrying.
    /// Nothing in the type system asks for this field, so only a test holds it.
    #[test]
    fn the_tools_reply_carries_a_freshness_ttl() {
        let raw = serde_json::to_value(server().tools_result()).expect("serialize");
        assert_eq!(raw["ttlMs"], 0, "reply was {raw}");
        assert_eq!(raw["cacheScope"], "private");
    }

    /// The registry is what the calling agent gets to choose from, so a repo
    /// key that isn't in it can't be expressed at all.
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

    /// Both tools have to be listed, or the one that isn't never reaches the
    /// agent.
    #[test]
    fn the_tools_reply_carries_both_tools() {
        let raw = serde_json::to_value(server().tools_result()).expect("serialize");
        let names: Vec<&str> = raw["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, vec![TOOL_CREATE_WORKSPACE, TOOL_LINK_PR]);
    }

    /// Only the reference is required: a workspace with one GitHub repo, or a
    /// pasted URL that names its own, gives Tethys enough to resolve the rest.
    #[test]
    fn link_pr_requires_only_the_reference() {
        let schema = serde_json::to_value(server().link_pr_schema()).expect("serialize");
        assert_eq!(schema["required"], serde_json::json!(["reference"]));
        assert_eq!(
            schema["properties"]["repo_key"]["enum"],
            serde_json::json!(["nl-frontend", "nl-backend"])
        );
    }

    /// Same rule as `repos`: an empty registry must not render an `enum` that
    /// nothing can satisfy.
    #[test]
    fn an_empty_registry_leaves_the_link_pr_enum_out() {
        let mut server = server();
        server.repo_keys.clear();
        let schema = serde_json::to_value(server.link_pr_schema()).expect("serialize");
        assert!(schema["properties"]["repo_key"]["enum"].is_null());
        assert_eq!(schema["properties"]["repo_key"]["type"], "string");
    }

    /// An empty registry means there is nothing to enumerate — the schema has
    /// to stay valid rather than offering an empty `enum` that matches nothing.
    #[test]
    fn an_empty_registry_leaves_the_enum_out() {
        let mut server = server();
        server.repo_keys.clear();
        let schema = serde_json::to_value(server.create_workspace_schema()).expect("serialize");
        assert!(schema["properties"]["repos"]["items"]["enum"].is_null());
        assert_eq!(schema["properties"]["repos"]["items"]["type"], "string");
    }
}
