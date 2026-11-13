# PRs

## Tracking

- A scan never removes a PR: tracking ends only at detach. Close the branch's PR and open another and you get two chips.
- Detach records the number in `dismissed`, or the next 45s scan would add the branch's PR straight back.

## Stacks

Stacks come from GitHub's `stack`/`stackEntry` fields, never from chaining base branches. A hand-made chain looks identical by base and isn't something `gh stack` manages.

A partial group shows the tracked positions (`2, 3 of 6`), not a count: `1 of 2` for a lone top PR read as a wrong position.

## Shipping

A Shipping workspace's "your turn" is its tile, not the turn highlight, because most of its turns are ones Tethys started. Auto-ship fires only when a PR gains a reviewer, so a workspace taken out of Shipping stays out until a PR is asked about again.
