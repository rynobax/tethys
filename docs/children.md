# Children

A handoff with `as_child` sets `parent` on the new workspace, once, never changed. It's separate from `Origin`, which is history and never acted on. The sidebar nests a child only when its parent is drawn too (`workspaceTree`). A parent can't be deleted while it has live children.

## The event

Deleting a child (the soft delete, not the purge) tells the direct parent: the branch and every PR it tracked or had detached, each fetched fresh (`child_events.rs`). Undo sends nothing. Codex parents are told nothing.

## Delivery

It uses Claude Code's cross-session inbox (`session_inbox.rs`, shared with Shipping). Each session exports `CLAUDE_CODE_MESSAGING_SOCKET` and `_TOKEN` to its hooks; `handle_session_start` stores them as `AgentSessionMeta::inbox`, matched by spawn token, so a nested `claude` in the same cwd can't take it. Persisted, because a reattached session fires no SessionStart. A post is an auth line with the token, then a user message; the token makes Claude deliver rather than hold it. An idle session starts a turn; a busy one reads it between tool calls.

Messages queue on `Workspace::pending_messages` before delivery and leave only once delivered. Fallbacks: the next replying hook's `additionalContext`, and a retry when a resumed session stores its new inbox. Returning it from the SessionStart hook was tried and is wrong: that context waits silently for Ryan's next prompt.

## Hush

A parent woken at rest by this news, with live children left, is hushed (`TurnTracker::hush`): it comes to rest already dismissed until Ryan types. It must stay in force past the Stop, because the probe often reads a just-stopped turn as working for a tick and a state change clears a dismissal. In memory only.

## Progress

`set_progress` draws done/total under the parent's name, whatever the agent says. Total 0 clears it.
