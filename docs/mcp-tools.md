# MCP tools

The config is rendered per session at spawn, nothing on disk. Claude gets `--mcp-config` plus `--allowed-tools` naming every tool, so nothing stalls on a permission prompt nobody sees. Never `--strict-mcp-config`, which cuts off the user's other servers. Codex gets `-c mcp_servers.tethys.…` with `approval_mode="approve"`. Identity is in the server's `env` block.

- `create_workspace` (handoff): returns once the draft exists; failures show only as a `CreationFailed` row. Inherits the caller's binary so work can't leave a `claude-hipaa` workspace by accident. `as_child` makes it a child (`docs/children.md`).
- `link_pr`: insert-or-refresh into the link's PR list; reports whether it's the branch's own PR.
- `add_repos`: same code as the UI picker (`provision::add_repos`). It waits, since the caller wants the path next; codex's `tool_timeout_sec` is raised for it. An interrupted call still finishes, so a repo already present is answered with its path rather than refused.
- `request_memory_budget`: `docs/sessions.md`. `leave_shipping`: `docs/prs.md`. `set_progress`: `docs/children.md`.

Trap: Claude Code's MCP protocol `2026-07-28` requires `ttlMs` on `tools/list`. `rmcp`'s `with_all_items` omits it and the reply is silently retried until "tools fetch failed". `tools_result()` sets it and a test holds it.
