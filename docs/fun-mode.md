# Fun mode

It's whimsy, so it must cost the real app nothing. three.js loads only when the toggle is first switched on, and nothing outside `src/fun/` and `avatar.rs` knows fun mode exists.

## Avatars

An avatar is a description, never an image, so a renderer can animate it. It's designed after the first turn, because the first prompt is often just a link.

- Always plain `claude` on the logged-in account, with `ANTHROPIC_API_KEY` stripped.
- A workspace on its own binary (`claude-hipaa`, `claude-unsafe`) is never designed. Its credentials mustn't be spent, and its words mustn't leave for the default account.

## Iterating

- `pnpm vite --port 5199`, then open `fun-preview.html`.
- `node scripts/fun-shot.mjs` takes screenshots at exact sim times; run it with no arguments for usage. Chrome's `--virtual-time-budget` was unreliable here.
- `pnpm dev:frozen` hot-reloads fun mode only.
