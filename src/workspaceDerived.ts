import type { ChecksRollup, GithubPrStatus, RepoLink, Workspace } from "./types";

/** Five minutes — matches the poller's stale threshold. */
const STALE_MS = 5 * 60 * 1000;

export function isStale(fetchedAt: string, nowMs: number = Date.now()): boolean {
  const t = new Date(fetchedAt).getTime();
  if (Number.isNaN(t)) return false;
  return nowMs - t > STALE_MS;
}

/**
 * Every PR tracked on a repo link: the branch-derived one plus any the user
 * attached by hand. Attached PRs with no status yet (first fetch failed) are
 * skipped — there's nothing to roll up.
 */
export function linkPrs(link: RepoLink): GithubPrStatus[] {
  const out: GithubPrStatus[] = [];
  if (link.github) out.push(link.github);
  for (const attached of link.attached_prs) {
    if (attached.status) out.push(attached.status);
  }
  return out;
}

/** Every PR tracked anywhere in the workspace. */
export function workspacePrs(ws: Workspace): GithubPrStatus[] {
  return ws.repo_links.flatMap(linkPrs);
}

/**
 * True when every PR tracked by the workspace — branch-derived and manually
 * attached — is merged. A workspace with no tracked PRs at all returns false
 * so we don't suggest deleting an unsynced workspace.
 */
export function isReadyToDelete(ws: Workspace): boolean {
  const prs = workspacePrs(ws);
  if (prs.length === 0) return false;
  return prs.every((pr) => pr.state === "merged");
}

/**
 * Worst-case rollup across all open PRs in the workspace.
 * Failure > Pending > Success. Neutral/None are ignored.
 */
export function checksSummary(ws: Workspace): ChecksRollup | null {
  let worst: ChecksRollup | null = null;
  for (const pr of workspacePrs(ws)) {
    if (pr.state !== "open") continue;
    if (pr.has_merge_conflicts) return "failure";
    const c = pr.checks;
    if (c === "failure") return "failure";
    if (c === "pending") worst = "pending";
    else if (c === "success" && worst === null) worst = "success";
  }
  return worst;
}

/** Sum of unresolved review threads across all open PRs. */
export function unresolvedTotal(ws: Workspace): number {
  let sum = 0;
  for (const pr of workspacePrs(ws)) {
    if (pr.state === "open") sum += pr.unresolved_threads;
  }
  return sum;
}

/** Find the primary PR chip to show on a workspace row — the first open PR, else first. */
export function primaryRepoLink(ws: Workspace): RepoLink | null {
  const open = ws.repo_links.find((r) => r.github?.state === "open");
  if (open) return open;
  return ws.repo_links.find((r) => r.github !== null) ?? null;
}
