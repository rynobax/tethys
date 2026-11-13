# Fun mode

A toggle swaps the sidebar for a 3D world (`src/fun/`) where each workspace is a resident whose place and behaviour show its state. It's whimsy, so it must cost the real app nothing: three.js loads only when the toggle is first switched on, and nothing outside `src/fun/` and `avatar.rs` knows it exists.

## Layers

- `residents.ts` reads Tethys state into a mood per workspace.
- `sim.ts` decides where everyone is and what they're doing on flat ground, and knows nothing about drawing.
- `Character.tsx` and `Scenery.tsx` draw it with react-three-fiber.

`stage.ts` lays the world out from the terminal pane's rectangle. The camera is orthographic and looks straight ahead, so screen x is world x. On a PR or artifact tab the pane widens over the right meadow, and `useStage` stops re-measuring so the world keeps its layout underneath.

## Behaviour worth knowing

- A Shipping workspace goes by its tile, not its turn.
- The selected resident stays put while selected, even through a mood change that would send it across a portal.
- Typing is heard through a capture-phase keydown, since the terminal stops the keys it handles.
- A deleted merged resident sails off before leaving (`Sim::sailors`, drawn from its last snapshot).
- The right-click menu is the sidebar's own `ContextMenu`, so the two can't drift.

## Avatars

An avatar is a description, never a picture, so any renderer can draw and animate it. `avatar.rs` designs one with `claude -p --model haiku --json-schema`, where the schema is the vocabulary, and `spec.ts` normalizes every field anyway. Design happens after the first turn, not the first prompt, since the prompt is often just a link.

- It always runs plain `claude` on the logged-in account, with `ANTHROPIC_API_KEY` stripped.
- A workspace on its own binary (`claude-hipaa`, `claude-unsafe`) is never designed: its credentials mustn't be spent and its words mustn't leave for the default account.
- Hooks are disabled and the cwd is outside every workspace, so turn tracking never sees it.

## Iterating

`pnpm vite --port 5199` and open `fun-preview.html` for every mood with real designs through `mockIPC`.

For screenshots, `node scripts/fun-shot.mjs` drives the preview headless. For example, `node scripts/fun-shot.mjs wait:8 mood:p=shipped wait:0.5 shot:/tmp/pop.png@40,520,200,150x3`; run it with no arguments for usage. `?shot` swaps in a manual clock and randomness is seeded, so a shot is of an exact sim time. Chrome's `--virtual-time-budget` was unreliable here.

`pnpm dev:frozen` hot-reloads fun mode and only fun mode (`funModeOnlyHmr` in `vite.config.ts`).
