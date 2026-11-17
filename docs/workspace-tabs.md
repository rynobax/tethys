# Workspace tabs

The terminal stays mounted on every tab, hidden with `visibility`, because unmounting would resize the tmux pane.

## PR tab

GitHub sends `frame-ancestors 'none'`, so a PR is a `<webview>`, not an iframe. It's a DOM element: dialogs and menus draw over it with no help.

- A `<webview>` reloads whenever its element is moved or removed, so `PrViews` renders every live one once, in a list it only appends to and evicts from, and lays each `position: fixed` over its tab's host. Hidden with `visibility`, since under `display: none` it loses its size.
- It sits inside `.detail` because fun mode makes that a stacking context; anywhere else it would cover the pane's own dialogs.
- No preload, and main refuses a `src` that isn't GitHub, so a page can't reach the bridge or Node.
- GitHub's Turbo links use `pushState`, which no navigation event sees, so leaving the PR is caught by a click handler main injects at `dom-ready`. Main also sends `window.open` and script navigations off GitHub to the browser, except mid sign-in, where an SSO provider takes over.

## Artifacts

An agent attaches them with `mcp__tethys__attach_artifact` and removes them with `detach_artifact`. Nothing is detected from hooks or output: guessing from Write calls, `open` commands and prose was unreliable.

- Attaching the same target again bumps its revision, which is what reloads the tab. Nothing watches the file.
- A URL must be on localhost, because a Review's iframe has no sandbox: ummon's server checks the request's origin.
- Pages load through `tethys-file:`, which serves only files under `worktree_root` (symlinks resolved), since agents write them. Attach refuses anything outside the workspace for the same reason.
- `net.fetch` answers a `file:` Range with the whole file as a 200, so `pages.ts` answers ranges itself with a 206. Without that, and the scheme's `stream` privilege, a large video won't start until it's read in full and can't seek.
- A Page's "Open in browser" is a daemon command: the bridge only hands http(s) URLs to the OS.
- `Workspace::artifacts` skips entries it can't read, so a stored diagram from before they were removed doesn't fail the whole of `state.json`.
