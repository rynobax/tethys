# Shipping

A workspace whose PRs need only approval, green CI and a merge goes into Shipping (`shipping.rs`, `Workspace::shipment`: `None`, `Shipping { prs }`, `Shipped`). It leaves the sidebar list for a tile grid; the tile, not the turn highlight, is its "your turn" signal, since most of its turns are ones Tethys started. Only the PRs named when shipping count.

- **Auto-ship:** when a PR goes from no reviewer to having one and every open PR now has one, the workspace ships. Edge-triggered, so a PR first seen with a reviewer ships nothing, and one taken out of Shipping stays out until a PR is asked about again.
- **Nudges:** `shipping::react` runs in the poller's store write over `StatusTransition`s. A Shipping PR that just went conflicting, red, or out of the merge queue unmerged queues a Nudge naming the fix. Starting to ship treats current status as new. Nudges use the children inbox (`docs/children.md`); `Shipper::wake` resumes an idle-closed session first. Codex gets none.
- **Shipped:** when every Shipping PR is closed. If another PR is still open it goes back to the list instead. Detaching every Shipping PR calls the shipment off. The agent can call `leave_shipping` when it needs Ryan.
