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

Each folder is a house front down the left meadow's outer edge, its name on a sign over the door, in the sidebar's order and as many as fit. A house is the folder's collapsed state, shared with the sidebar: shut, its folk walk home and go in, and the windows light up, except anyone who needs you, who waits out front where you can see it; open, they come out the door and go wherever they belong. Clicking the house opens or shuts the folder. The selected workspace stays out whatever its folder, and so does anyone shipping or shipped.

## Shipping

A Shipping workspace waiting on other people pitches to the crowd on the stage. Approved and green, it lines up at the ticket office for Ryan to merge it; in the merge queue, it waits on the quay above the pier, parcel in hand, for its turn on the boat. Merged, it pops its champagne and joins the line along the pier.

## Waiting

Anyone who needs you picks its own place on the left meadow, loosely near the middle with room for its tag, and keeps it until it's dealt with. A fixed grid of slots read as a grid, and filling the best free slot had others walk over to take a place just left. Past the room, it goes where it covers others' tags least, never off the waiting ground, so never by the stage.

## Walking

Everyone finds their way across a meadow on a grid of its walkable ground, baked per layout (`paths.ts`), rather than steering round what's ahead: steering wedged walkers in corners and between furniture. A spot off walkable ground, a seat or a ring on the pool, is reached by a straight step from the nearest cell on the side it's coming from.

Someone settled is no wall to a walker, who passes by; only those standing about make way. Pushing settled ones about is what wedged walkers in narrow gaps like the ticket line.

Anyone making no headway for 3 s finds a new way, and stuck after that is put on its target with a `console.warn`. That's a bug, so it's never meant to happen: `node scripts/fun-walk.mjs` walks everyone between every named spot and runs a crowd through an hour of churn, and fails on any warning. Run it after changing the layout or how anyone moves.

## Environments

Each day is spent somewhere: the meadow, a moon base, Athens or an office floor (`environment.ts`). The day picks it, not a random draw kept somewhere: days run in fours with every environment once, shuffled per four, and a four never starts on the one the last ended on. Midnight moves it on while the app's open.

Everything happens the same everywhere, on the same layout; only what it's drawn as changes. The pool is a ball pit in the office, the boat a shuttle, a galley or a rubber duck, the houses habitat modules, temples or meeting rooms, and the residents put on a bubble helmet, a laurel wreath or a tie. Each environment has a light and a dark palette (`palette.ts`), and its own props in `environments/`; the meadow's are where they always were. Who draws what is a switch on the environment at each place it differs, read from `EnvironmentContext`.

The office is indoors and the moon has no air, so neither has the weather; the moon has meteors instead. The moon has low gravity too: everyone bounds about rather than walking, and every hop is a slow arc that hangs at the top (`hop` in `Character.tsx`).

View → Next environment steps on to the next, to look round them all, until a reload or midnight. The menu sends `next-environment` and the preload passes it on as a `tethys:next-environment` DOM event, so neither knows more of fun mode than that.

The preview takes `?environment=moon` (or any of `ENVIRONMENTS`), as does the gallery, to see the outfits; a shot is in the meadow unless asked.

## Weather

The meadows have the weather where Ryan is: found by IP on geojs, looked up on Open-Meteo every 15 minutes, neither wanting a key (`forecast.ts`). IP rather than `navigator.geolocation`, which would ask macOS for location access for a city-level answer. The last look is kept in `localStorage`, so a restart opens on it.

Cloud dims the sun and greys the sky, broken cloud drifts shadows across the ground with the wind, rain darkens the grass and splashes, snow lies on it, fog thickens toward the back, and a storm flashes lightning, all by reweathering the `Environment` plus `Weather.tsx`. Screen up is north, so the wind blows rain and cloud the way it really is. Rain and snow clamp their step to 0.1 s: one long frame, back from hidden or off a manual clock, would bury every drop.

The preview takes `?weather=rain` (or any of `PRESETS`); a shot is clear unless asked, so shots don't depend on the sky.

## Iterating

- `pnpm vite --port 5199`, then open `fun-preview.html` in any browser, or `fun-preview.html?gallery` for every animal side by side. It installs a stand-in for the preload's bridge that answers every command with null.
- `node scripts/fun-shot.mjs` takes screenshots at exact sim times; run it with no arguments for usage. Chrome's `--virtual-time-budget` was unreliable here.
- `pnpm dev:frozen` hot-reloads fun mode only.
