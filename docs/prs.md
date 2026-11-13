# PRs

## Tracking

A repo link tracks one list, `RepoLink::prs`. The branch's own PR is in it like any other; it's only added for you. It used to have its own slot, which gave it accidental special behaviour, and `migrate_pr_slots` folded that away.

The poller's `TargetKind::Branch` is a scan that finds which PR to track; `TargetKind::Pr` polls every tracked PR. A tick scans, then fetches what the scan just added, so a new PR doesn't sit on "no data" for 45s.

- A scan never removes anything: close the branch's PR and open another and you get two chips.
- Detach records the number in `dismissed`, or the next scan would add it straight back. Attaching by number clears it.

## Stacks

Members of a `gh stack` are outlined together in the tab strip, base first. Membership is GitHub's own (`stack { number size }`, `stackEntry { position }`); `parse_stack` needs all three or yields `None`. Grouping by base branch would be wrong: a hand-made chain looks identical and isn't something `gh stack` manages. Nothing fetches `baseRefName`.

A group smaller than the stack shows the tracked positions, `2, 3 of 6`, never a count, which read like a position.

## Shipping

A workspace whose PRs need only approval, green CI and a merge goes into Shipping (`shipping.rs`, `Workspace::shipment`: `None`, `Shipping { prs }`, `Shipped`). It leaves the sidebar list for a tile grid; the tile, not the turn highlight, is its "your turn" signal, since most of its turns are ones Tethys started. Only the PRs named when shipping count.

- **Auto-ship:** when a PR goes from no reviewer to having one and every open PR now has one, the workspace ships. Edge-triggered, so a PR first seen with a reviewer ships nothing, and one taken out of Shipping stays out until a PR is asked about again.
- **Nudges:** `shipping::react` runs in the poller's store write over `StatusTransition`s. A Shipping PR that just went conflicting, red, or out of the merge queue unmerged queues a Nudge naming the fix. Starting to ship treats current status as new. Nudges use the children inbox (`docs/children.md`); `Shipper::wake` resumes an idle-closed session first. Codex gets none.
- **Shipped:** when every Shipping PR is closed. If another PR is still open it goes back to the list instead. Detaching every Shipping PR calls the shipment off. The agent can call `leave_shipping` when it needs Ryan.
