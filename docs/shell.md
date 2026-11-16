# Shell and daemon

Electron (`electron/`) is the shell. The Rust core is `tethysd`, which main spawns and every page talks to over `ui.sock`.

- The preload is the daemon's client, so a connection lives exactly as long as its page: a reload drops it, and the daemon lets go of that page's terminal feeds. It's why the main window isn't sandboxed. Main keeps a second connection for the menu, window focus and Pages' scope.
- The daemon dies with the shell by reading EOF on stdin, so its stdin must stay a pipe nothing writes to. `/dev/null` there stops it at once.
- Requests run concurrently and replies come back in any order. PTY commands (`ORDERED` in `ui_socket.rs`) are the exception and share one queue per connection, or a detach could overtake its attach and a drag's last resize could land early.
- `attach_session` sends scrollback as the channel's first frame, not in its reply. On one ordered socket a live chunk could otherwise land ahead of it.
- A channel argument crosses as a number the page allocates. Top-level argument keys are camelCase and keys inside an `args` struct snake_case, which is Tauri's convention, kept so the wrappers in `src/ipc/commands.ts` didn't change.
- `tethysd` finds `tethys-hook` and `tethys-mcp` beside its own executable, so packaging copies all three into `Contents/MacOS/`.
