# Workspace tabs

Chat, one tab per tracked PR, one per artifact (`WorkspaceTabs.tsx`). The terminal stays mounted on every tab, hidden with `visibility`, so switching never resizes the tmux pane; its input is blurred while hidden.

## PR tab

GitHub sends `frame-ancestors 'none'`, so a PR is a native child webview (`pr_view.rs`, Tauri's `unstable` multi-webview), not an iframe. Consequences:

- The frontend owns geometry: `PrView` sends its rect on every layout change.
- It's above every DOM layer, so `PrView` hides it while a `.modal-backdrop` exists or a `.context-menu` overlaps it.
- Capabilities are scoped to the `main` webview, but app commands aren't ACL-gated, so the real safeguard is only ever loading GitHub.
- One webview per PR URL, hidden rather than renavigated, LRU-capped (`MAX_LIVE`).
- Links leaving the PR open in the browser via a page-side click handler, since Turbo links never reach the navigation handler.

## Artifacts

Artifacts come from hooks, not the screen (`artifacts.rs`). Claude draws on the alternate screen, so xterm's buffer never holds a whole diagram.

- **Diagram:** mermaid fences in the `Stop` hook's `last_assistant_message`. Only the final text block, so a diagram before a tool call is missed.
- **Page:** a `.html` path from `Write`/`Edit`/`MultiEdit`, or a Bash `open` of one (`/show-me` writes with a heredoc and ends with `open`). Also read off `PostToolUseFailure` and off a final message asking Ryan to `! open` it, since sandboxed sessions can't. Paths are canonicalized so the write and the open are one tab.
- **Review:** the `ummon is reviewing <repo> at <url>` line in a tool's last 8KB of stdout. Iframed without a sandbox because ummon's server checks the origin.

Pages and reviews must resolve under the workspace root. Seeing the same thing again bumps `revision` (which the iframes key on) instead of adding a tab. Persisted on `Workspace::artifacts`, capped at 12. Writes use `update_workspace_quiet` so the sidebar doesn't refetch.

Pages render in a sandboxed iframe over the asset protocol, whose scope is empty statically and opened to `worktree_root` at boot. "Open in browser" is a Rust command because plugin-opener's `openPath` is scoped to fixed paths.
