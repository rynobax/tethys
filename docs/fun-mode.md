# Fun mode

It's whimsy, so it must cost the real app nothing. three.js loads only when the toggle is first switched on, and nothing outside `src/fun/` knows fun mode exists.

## Animals

Each family is one animal: a child is always its parent's, so a family reads as one at a glance across the meadows.

- A new top-level workspace gets an animal at random that no living workspace is. Past 20 families, one of the least used.
- The choice is kept in the webview's `localStorage`, so it survives restarts but isn't Tethys state.
- Every animal is drawn on one rig (`animals/rig.tsx`): it draws its head, torso, arms, feet and tail at shared joints, and every pose and held thing works for all of them. A new animal never touches `Character.tsx`.

## Iterating

- `pnpm vite --port 5199`, then open `fun-preview.html`, or `fun-preview.html?gallery` for every animal side by side.
- `node scripts/fun-shot.mjs` takes screenshots at exact sim times; run it with no arguments for usage. Chrome's `--virtual-time-budget` was unreliable here.
- `pnpm dev:frozen` hot-reloads fun mode only.
