# MCP tools

- Claude gets `--allowed-tools` naming every tool, so no call stalls on a prompt nobody sees. Never `--strict-mcp-config`, which would cut the user's other servers.
- A handoff inherits the caller's binary, so work can't leave a `claude-hipaa` workspace by accident.
- An interrupted `add_repos` still finishes. A repo the workspace already has is answered with its path rather than refused, so a retry succeeds.
- Claude Code's MCP protocol `2026-07-28` requires `ttlMs` on `tools/list`. `rmcp`'s `with_all_items` omits it, and the reply is silently retried until "tools fetch failed". A test holds `tools_result()` to it.
