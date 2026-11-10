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
    read_frame, write_frame, AddRepos, CreateWorkspace, LeaveShipping, LinkPr, Request,
    RequestMemoryBudget, Response, ENV_REPO_KEYS, ENV_SESSION_ID, ENV_SOCKET, ENV_WORKSPACE_ID,
    TOOL_ADD_REPOS, TOOL_CREATE_WORKSPACE, TOOL_LEAVE_SHIPPING, TOOL_LINK_PR,
    TOOL_REQUEST_MEMORY_BUDGET,
};

#[derive(Debug, Deserialize)]
struct CreateWorkspaceArgs {
    repos: Vec<String>,
    branch: String,
    brief: String,
    #[serde(default)]
    as_child: bool,
}

#[derive(Debug, Deserialize)]
struct LinkPrArgs {
    reference: String,
    #[serde(default)]
    repo_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddReposArgs {
    repos: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RequestMemoryBudgetArgs {
    gb: u64,
    minutes: u64,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct LeaveShippingArgs {
    reason: String,
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
    fn repo_key_items(&self) -> serde_json::Value {
        if self.repo_keys.is_empty() {
            json!({ "type": "string" })
        } else {
            json!({ "type": "string", "enum": self.repo_keys })
        }
    }

    fn create_workspace_schema(&self) -> JsonObject {
        let schema = json!({
            "type": "object",
            "properties": {
                "repos": {
                    "type": "array",
                    "items": self.repo_key_items(),
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
                "as_child": {
                    "type": "boolean",
                    "description": "Set true if you need to be notified when this \
                        workspace finishes its work. Default to false if unsure.",
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
                 anything further — so put everything it needs in the brief. The \
                 one exception is `as_child`, which tells you when the child is \
                 deleted.\n\n\
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

    fn add_repos_schema(&self) -> JsonObject {
        let schema = json!({
            "type": "object",
            "properties": {
                "repos": {
                    "type": "array",
                    "items": self.repo_key_items(),
                    "minItems": 1,
                    "description": "Repo keys to add to this workspace. Each \
                        becomes a git worktree on this workspace's branch, beside \
                        the repos already here.",
                },
            },
            "required": ["repos"],
            "additionalProperties": false,
        });
        schema
            .as_object()
            .cloned()
            .expect("input schema literal is an object")
    }

    fn add_repos_tool(&self) -> Tool {
        Tool::new(
            Cow::Borrowed(TOOL_ADD_REPOS),
            Cow::Borrowed(
                "Add repos to the Tethys workspace this session belongs to. Each \
                 one gets a git worktree at the workspace root, checked out on this \
                 workspace's branch and set up the same way as the repos already \
                 here — clone, dependency install, setup scripts.\n\n\
                 Reach for it when the task needs a repo this workspace doesn't \
                 have. Don't read another checkout of it, clone it yourself, or \
                 guess at its contents instead.\n\n\
                 The call waits until every repo is ready and returns their \
                 worktree paths. That takes minutes, and longer if Tethys is busy \
                 setting up other workspaces, since it builds one at a time. Repos \
                 already in the workspace are reported back rather than refused. \
                 If one fails, the repos added before it stay added.",
            ),
            self.add_repos_schema(),
        )
    }

    fn request_memory_budget_schema(&self) -> JsonObject {
        let schema = json!({
            "type": "object",
            "properties": {
                "gb": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "The budget to ask for, in GB, covering everything \
                        this session runs at once. Ask for what the run needs, not \
                        the most you can think of.",
                },
                "minutes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 480,
                    "description": "How long the higher budget lasts before the \
                        normal one applies again.",
                },
                "reason": {
                    "type": "string",
                    "description": "Shown to Ryan when he decides: what you need \
                        to run and why it needs this much memory.",
                },
            },
            "required": ["gb", "minutes", "reason"],
            "additionalProperties": false,
        });
        schema
            .as_object()
            .cloned()
            .expect("input schema literal is an object")
    }

    fn request_memory_budget_tool(&self) -> Tool {
        Tool::new(
            Cow::Borrowed(TOOL_REQUEST_MEMORY_BUDGET),
            Cow::Borrowed(
                "Ask Ryan to raise this session's memory budget for a while. \
                 Tethys kills the biggest job a session is running once \
                 everything it started holds more memory than its budget, and \
                 tells you when it has.\n\n\
                 Reach for it only after a kill, when the run genuinely needs the \
                 memory — a narrower run (fewer test files, fewer workers) is \
                 usually the better answer. Ryan approves or denies it in \
                 Tethys, and the call waits for him, up to 30 minutes. \
                 Approval applies to this session only, and only until the time \
                 runs out.",
            ),
            self.request_memory_budget_schema(),
        )
    }

    fn leave_shipping_schema(&self) -> JsonObject {
        let schema = json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "One line, shown to Ryan: what needs his input.",
                },
            },
            "required": ["reason"],
            "additionalProperties": false,
        });
        schema
            .as_object()
            .cloned()
            .expect("input schema literal is an object")
    }

    fn leave_shipping_tool(&self) -> Tool {
        Tool::new(
            Cow::Borrowed(TOOL_LEAVE_SHIPPING),
            Cow::Borrowed(
                "Take this workspace out of Shipping and hand it back to Ryan. \
                 A workspace is Shipping once Ryan considers its work done: \
                 Tethys watches its PRs until they merge and messages this \
                 session about anything it should fix alone, like a merge \
                 conflict or failing CI.\n\n\
                 Call this when something needs Ryan's input instead: a design \
                 question, a review comment he should weigh in on, anything you \
                 can't settle alone. Then end your turn explaining what you need. \
                 Only meaningful while the workspace is Shipping.",
            ),
            self.leave_shipping_schema(),
        )
    }

    /// Protocol `2026-07-28` requires `ttlMs`, which `with_all_items` omits;
    /// without it Claude Code retries until "tools fetch failed". Zero, since
    /// the `repos` enum is fixed at spawn.
    fn tools_result(&self) -> ListToolsResult {
        ListToolsResult::with_all_items(vec![
            self.create_workspace_tool(),
            self.link_pr_tool(),
            self.add_repos_tool(),
            self.request_memory_budget_tool(),
            self.leave_shipping_tool(),
        ])
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
                 workspace with its own branch and its own session, and add_repos \
                 when the work in this one needs a repo it doesn't have yet.",
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
            TOOL_ADD_REPOS => self.add_repos(request.arguments).await,
            TOOL_REQUEST_MEMORY_BUDGET => self.request_memory_budget(request.arguments).await,
            TOOL_LEAVE_SHIPPING => self.leave_shipping(request.arguments).await,
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
            as_child: args.as_child,
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

impl TethysServer {
    async fn add_repos(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args: AddReposArgs = parse_args(arguments)?;
        let request = Request::AddRepos(AddRepos {
            from_workspace: self.from_workspace.clone(),
            from_session: self.from_session.clone(),
            repos: args.repos,
        });

        let response = match self.send(&request).await {
            Ok(response) => response,
            Err(e) => return Ok(failed(format!("adding repos failed: {e}"))),
        };

        Ok(match response {
            Response::Added { branch, repos } => {
                let lines: Vec<String> = repos
                    .iter()
                    .map(|r| {
                        let note = if r.added { "" } else { " (already here)" };
                        format!("- {}: {}{note}", r.repo_key, r.worktree_path)
                    })
                    .collect();
                CallToolResult::success(vec![ContentBlock::text(format!(
                    "Ready, on branch `{branch}`:\n{}",
                    lines.join("\n")
                ))])
                .into()
            }
            Response::Rejected { message } => failed(format!("adding repos failed: {message}")),
            other => failed(format!("Tethys answered an add with {other:?}")),
        })
    }
}

impl TethysServer {
    async fn request_memory_budget(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args: RequestMemoryBudgetArgs = parse_args(arguments)?;
        let request = Request::RequestMemoryBudget(RequestMemoryBudget {
            from_workspace: self.from_workspace.clone(),
            from_session: self.from_session.clone(),
            gb: args.gb,
            minutes: args.minutes,
            reason: args.reason,
        });

        let response = match self.send(&request).await {
            Ok(response) => response,
            Err(e) => return Ok(failed(format!("budget request failed: {e}"))),
        };

        Ok(match response {
            Response::BudgetApproved { gb, until } => {
                CallToolResult::success(vec![ContentBlock::text(format!(
                    "Approved: this session's memory budget is {gb} GB until {until}, \
                     then the normal budget applies again."
                ))])
                .into()
            }
            Response::BudgetDenied => CallToolResult::success(vec![ContentBlock::text(
                "Denied: the budget is unchanged. Don't rerun the job as it was; \
                 narrow it, or ask Ryan how he'd like to proceed."
                    .to_string(),
            )])
            .into(),
            Response::Rejected { message } => {
                failed(format!("budget request refused, nothing changed: {message}"))
            }
            other => failed(format!("Tethys answered a budget request with {other:?}")),
        })
    }
}

impl TethysServer {
    async fn leave_shipping(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args: LeaveShippingArgs = parse_args(arguments)?;
        let request = Request::LeaveShipping(LeaveShipping {
            from_workspace: self.from_workspace.clone(),
            from_session: self.from_session.clone(),
            reason: args.reason,
        });

        let response = match self.send(&request).await {
            Ok(response) => response,
            Err(e) => return Ok(failed(format!(
                "leaving Shipping failed, the workspace is still Shipping: {e}"
            ))),
        };

        Ok(match response {
            Response::LeftShipping => CallToolResult::success(vec![ContentBlock::text(
                "The workspace is no longer Shipping and is back with Ryan. End your \
                 turn telling him what you need."
                    .to_string(),
            )])
            .into(),
            Response::Rejected { message } => failed(format!(
                "couldn't leave Shipping: {message}"
            )),
            other => failed(format!("Tethys answered leave_shipping with {other:?}")),
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

    #[test]
    fn add_repos_enumerates_the_registry() {
        let schema = serde_json::to_value(server().add_repos_schema()).expect("serialize");
        assert_eq!(schema["required"], serde_json::json!(["repos"]));
        assert_eq!(
            schema["properties"]["repos"]["items"]["enum"],
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
