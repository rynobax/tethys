# PRs

## Tracking

A repo link tracks one list, `RepoLink::prs`. The branch's own PR is in it like any other; it's only added for you. It used to have its own slot, which gave it accidental special behaviour, and `migrate_pr_slots` folded that away.

The poller's `TargetKind::Branch` is a scan that finds which PR to track; `TargetKind::Pr` polls every tracked PR. A tick scans, then fetches what the scan just added, so a new PR doesn't sit on "no data" for 45s.

- A scan never removes anything: close the branch's PR and open another and you get two chips.
- Detach records the number in `dismissed`, or the next scan would add it straight back. Attaching by number clears it.

## Stacks

Members of a `gh stack` are outlined together in the tab strip, base first. Membership is GitHub's own (`stack { number size }`, `stackEntry { position }`); `parse_stack` needs all three or yields `None`. Grouping by base branch would be wrong: a hand-made chain looks identical and isn't something `gh stack` manages. Nothing fetches `baseRefName`.

A group smaller than the stack shows the tracked positions, `2, 3 of 6`, never a count, which read like a position.
