# Workspace tabs

The terminal stays mounted on every tab, hidden with `visibility`, because unmounting would resize the tmux pane.

## PR tab

GitHub sends `frame-ancestors 'none'`, so a PR is a `<webview>`, not an iframe. It's a DOM element: dialogs and menus draw over it with no help.

- A `<webview>` reloads whenever its element is moved or removed, so `PrViews` renders every live one once, in a list it only appends to and evicts from, and lays each `position: fixed` over its tab's host. Hidden with `visibility`, since under `display: none` it loses its size.
- It sits inside `.detail` because fun mode makes that a stacking context; anywhere else it would cover the pane's own dialogs.
- No preload, and main refuses a `src` that isn't GitHub, so a page can't reach the bridge or Node.
- GitHub's Turbo links use `pushState`, which no navigation event sees, so leaving the PR is caught by a click handler main injects at `dom-ready`. Main also sends `window.open` and script navigations off GitHub to the browser, except mid sign-in, where an SSO provider takes over.

## Artifacts

Artifacts come from hooks, not xterm's buffer. Claude draws on the alternate screen, so the buffer only ever holds the visible part of a diagram.

- `last_assistant_message` is only the turn's final text block, so a diagram before a tool call is missed.
- Pages are also caught by a Bash `open`, because `/show-me` writes with a heredoc the file tools never see. Sandboxed sessions can't `open`, so it's also read off `PostToolUseFailure` and off a final message asking for `! open`.
- A Review's iframe has no sandbox, because ummon's server checks the request's origin.
- Pages load through `tethys-file:`, which serves only files under `worktree_root` (symlinks resolved), since agents write them.
- A Page's "Open in browser" is a daemon command: the bridge only hands http(s) URLs to the OS.
