import type { GithubPrStatus, RepoLink, Workspace } from "./types";

/**
 * Five minutes — several missed ticks at the poller's 45s base interval. Past
 * this we stop trusting the numbers and fade the chip: polling is wedged, or
 * backed off far enough that it may as well be.
 */
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
