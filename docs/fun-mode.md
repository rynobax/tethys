# Fun mode

It's whimsy, so it must cost the real app nothing. three.js loads only when the toggle is first switched on, and nothing outside `src/fun/` knows fun mode exists.

## Animals

Each family is one animal: a child is always its parent's, so a family reads as one at a glance across the meadows.

- A new top-level workspace gets an animal at random that no living workspace is. Past 20 families, one of the least used.
- The choice is kept in the page's `localStorage`, so it survives restarts but isn't Tethys state.
- Every animal is drawn on one rig (`animals/rig.tsx`): it draws its head, torso, arms, feet and tail at shared joints, and every pose and held thing works for all of them. A new animal never touches `Character.tsx`.

## Parents

A workspace with children, or with a task count, quilts on the porch along the right meadow's front while it's idle or working. Its quilt has a square per task, sewn in its family's colours as each is done, and it cheers from its chair each time one is. Any other mood sends it where that mood always does, and with every place on the porch taken it does as anyone else would.

Hovering a parent, or its tag, rings it and everyone it handed work to, down the generations, in its family's colour, and outlines their tags in it, so its children can be found wherever they are.

## Folders

Each folder is a house front down the left meadow's outer edge, its name on a sign over the door, in the sidebar's order and as many as fit. A house is the folder's collapsed state, shared with the sidebar: shut, its folk walk home and go in, and the windows light up, except anyone who needs you, who waits out front where you can see it; open, they come out the door and go wherever they belong. Clicking the house opens or shuts the folder. The selected workspace stays out whatever its folder.

## Iterating

- `pnpm vite --port 5199`, then open `fun-preview.html` in any browser, or `fun-preview.html?gallery` for every animal side by side. It installs a stand-in for the preload's bridge that answers every command with null.
- `node scripts/fun-shot.mjs` takes screenshots at exact sim times; run it with no arguments for usage. Chrome's `--virtual-time-budget` was unreliable here.
- `pnpm dev:frozen` hot-reloads fun mode only.
