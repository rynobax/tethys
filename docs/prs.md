# PRs

## Tracking

- A scan never removes a PR: tracking ends only at detach. Close the branch's PR and open another and you get two chips.
- Detach records the number in `dismissed`, or the next 45s scan would add the branch's PR straight back.

## Polling

- A write from a PR's tab, such as a review request or joining the merge queue, polls that PR a second later rather than waiting up to a minute for the next tick. Electron sees it as any request but a GET from the tab's partition, less the read-only POSTs in `electron/pr-view.ts`'s `NOT_WRITES`. The daemon logs each such poll with the request that caused it: one that shows up without a click is a read to add there.
- A PR poll that finds nothing new tries once more 4s later, in case the write isn't readable yet.
- An answer is stamped with when it was asked for, and an older one never replaces a newer one. A 6s tick that started before the click would otherwise undo the PR poll's answer and fire edge-triggered reactions twice.

## Stacks

Stacks come from GitHub's `stack`/`stackEntry` fields, never from chaining base branches. A hand-made chain looks identical by base and isn't something `gh stack` manages.

A partial group shows the tracked positions (`2, 3 of 6`), not a count: `1 of 2` for a lone top PR read as a wrong position.

## Shipping

A Shipping workspace's "your turn" is its tile, not the turn highlight, because most of its turns are ones Tethys started. Auto-ship fires only when a PR gains a reviewer, so a workspace taken out of Shipping stays out until a PR is asked about again.
