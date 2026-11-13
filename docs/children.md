# Children

## The session inbox

Delivery uses Claude Code's cross-session inbox:

- Each session exports `CLAUDE_CODE_MESSAGING_SOCKET` and `_TOKEN` to its hooks.
- A post is `{"type":"auth","token":…}` then `{"type":"user","message":{"role":"user","content":…}}`. The token makes Claude deliver it rather than hold it for approval.
- An idle session starts a turn with it; a busy one reads it between tool calls.

The details that matter:

- The inbox is stored only from SessionStart with an exact spawn-token match, because a nested `claude` shares the cwd but has its own inbox. It's persisted because a reattached session fires no SessionStart.
- Don't return queued news from the SessionStart hook. That context waits silently for Ryan's next prompt; post it once the new inbox is stored instead.
- A hush must stay in force past the Stop. The probe often reads a just-stopped turn as working for a tick, and any probe change clears a one-off dismissal.
