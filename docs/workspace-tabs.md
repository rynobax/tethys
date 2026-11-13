# Workspace tabs

The terminal stays mounted on every tab, hidden with `visibility`, because unmounting would resize the tmux pane.

## PR tab

GitHub sends `frame-ancestors 'none'`, so a PR is a native child webview, not an iframe.

- It draws above every DOM layer, so anything that must cover it (dialogs, context menus) has to hide it.
- App commands aren't ACL-gated, so the only real safeguard is that it only ever loads GitHub.
- GitHub's Turbo links never reach the navigation handler, so leaving the PR is caught by a page-side click handler.

## Artifacts

Artifacts come from hooks, not xterm's buffer. Claude draws on the alternate screen, so the buffer only ever holds the visible part of a diagram.

- `last_assistant_message` is only the turn's final text block, so a diagram before a tool call is missed.
- Pages are also caught by a Bash `open`, because `/show-me` writes with a heredoc the file tools never see. Sandboxed sessions can't `open`, so it's also read off `PostToolUseFailure` and off a final message asking for `! open`.
- A Review's iframe has no sandbox, because ummon's server checks the request's origin.
- "Open in browser" is a Rust command, because plugin-opener's `openPath` is scoped to paths fixed in the capability file.
