import type {
  Folder,
  GithubPrStatus,
  RepoLink,
  Workspace,
  WorkspaceId,
} from "./types";

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
 * A tracked PR with a status to draw, plus its number.
 *
 * The number used to be nullable, because the PR for the workspace's own branch
 * lived in a slot of its own and was the one PR you couldn't detach. Every
 * tracked PR now has a number and every one of them can be detached, so this is
 * just a `GithubPrStatus` that hasn't lost track of which entry it belongs to
 * through the regrouping below.
 */
export type LinkPr = {
  status: GithubPrStatus;
  number: number;
};

/**
 * Every PR tracked on a repo link that has something to draw, in the order
 * tracking started. PRs with no status yet (a first fetch that failed) are
 * skipped — there's nothing to roll up.
 */
export function linkPrEntries(link: RepoLink): LinkPr[] {
  const out: LinkPr[] = [];
  for (const pr of link.prs) {
    if (pr.status) out.push({ status: pr.status, number: pr.number });
  }
  return out;
}

export function linkPrs(link: RepoLink): GithubPrStatus[] {
  return linkPrEntries(link).map((e) => e.status);
}

/**
 * A repo's PRs as the header draws them: either one `gh stack` and the members
 * of it this workspace tracks, or a single PR that isn't in a stack.
 */
export type PrGroup = {
  /** The stack these PRs belong to; `null` for a PR in no stack at all. */
  stack: { number: number; size: number } | null;
  /** Stack members in position order, base-first. */
  prs: LinkPr[];
};

/**
 * Partitions a repo link's PRs into groups: one per `gh stack` present, plus a
 * group of one for every PR that isn't in a stack. Every PR comes back exactly
 * once, so the header can render groups uniformly.
 *
 * Membership is GitHub's own — `stack` is only set once `gh stack` has made the
 * stack a real object on GitHub's side, so PRs merely based on each other by
 * hand stay separate chips. That's deliberate: a manual chain and a `gh stack`
 * look identical from the base branches alone, and only one of them is a thing
 * you can `gh stack sync`.
 *
 * A group can be smaller than `stack.size` — a stack of six with one branch
 * checked out here shows one chip — so callers wanting "is this the whole
 * stack" have to compare the two.
 *
 * Stack numbers are per repository and a link is one repo, so there's nothing
 * here that could pull two repos' PRs into a group.
 */
export function prGroups(entries: LinkPr[]): PrGroup[] {
  const groups: PrGroup[] = [];
  // Keyed by stack number, so a group lands where its first member appeared
  // and chips don't jump when an unrelated PR is attached.
  const byStack = new Map<number, PrGroup>();

  for (const entry of entries) {
    const stack = entry.status.stack;
    if (!stack) {
      groups.push({ stack: null, prs: [entry] });
      continue;
    }
    const existing = byStack.get(stack.number);
    if (existing) {
      existing.prs.push(entry);
      continue;
    }
    const group: PrGroup = {
      stack: { number: stack.number, size: stack.size },
      prs: [entry],
    };
    byStack.set(stack.number, group);
    groups.push(group);
  }

  for (const group of byStack.values()) {
    // Position is 1-based from the stack's base branch. Ties can't happen on
    // GitHub's side, and sort is stable, so a hand-edited file that repeats a
    // position just keeps the order it was listed in.
    group.prs.sort((a, b) => a.status.stack!.position - b.status.stack!.position);
  }

  return groups;
}

/** A workspace plus how deep it sits in the blocker tree. */
export type WorkspaceRow = { workspace: Workspace; depth: number };

/**
 * Flatten one folder's workspaces into the order the sidebar draws them, with
 * a blocked workspace tucked under the one it's waiting on.
 *
 * Blocked-ness is decided *here* rather than stored, so it can only ever mean
 * "my blocker is one of the rows I'm being drawn with". Soft-deleting a
 * blocker drops it out of `workspaces`, and moving it to another folder drops
 * it out of *this* call, which un-nests everything waiting on it without
 * touching a single `blocked_by` field — and undoing either brings the nesting
 * straight back. That the folder boundary works this way is why `folderSections`
 * calls this once per folder rather than passing folders in.
 *
 * Fan-out is ordinary: a blocker holding up three workspaces gets three
 * children. Fan-in can't happen, because `blocked_by` is a single field.
 *
 * Ties into ordering by leaving roots in the order given (the manual
 * drag order) and placing children right after their parent. A cycle
 * surviving in `state.json` is treated as unparented rather than followed,
 * so this always terminates and every workspace appears exactly once.
 */
export function workspaceTree(workspaces: Workspace[]): WorkspaceRow[] {
  const present = new Set(workspaces.map((w) => w.id));
  const childrenOf = new Map<string, Workspace[]>();
  const roots: Workspace[] = [];

  for (const w of workspaces) {
    // A blocker that isn't on screen doesn't block; nor does a self-link.
    const parent =
      w.blocked_by && w.blocked_by !== w.id && present.has(w.blocked_by)
        ? w.blocked_by
        : null;
    if (parent) {
      const siblings = childrenOf.get(parent);
      if (siblings) siblings.push(w);
      else childrenOf.set(parent, [w]);
    } else {
      roots.push(w);
    }
  }

  const out: WorkspaceRow[] = [];
  const seen = new Set<string>();
  const walk = (w: Workspace, depth: number) => {
    if (seen.has(w.id)) return;
    seen.add(w.id);
    out.push({ workspace: w, depth });
    for (const child of childrenOf.get(w.id) ?? []) walk(child, depth + 1);
  };
  for (const root of roots) walk(root, 0);

  // Anything still unvisited is in a cycle among themselves — no root can
  // reach it. Surface those flat rather than dropping them from the sidebar.
  for (const w of workspaces) if (!seen.has(w.id)) walk(w, 0);

  return out;
}

/** A folder and the rows the sidebar draws under it. `folder: null` is the
 *  Default folder — the absence of a folder rather than one of them. */
export type FolderSection = { folder: Folder | null; rows: WorkspaceRow[] };

/**
 * Cut the sidebar's workspaces into their folders: Default first, then the
 * folders in their stored order, each one's rows nested by blocker.
 *
 * Empty sections are kept — an empty folder still needs a header to drop onto.
 * A workspace naming a folder that isn't in `folders` lands in Default, which
 * mirrors the boot-time prune in Rust rather than making the row disappear
 * until the next restart.
 */
export function folderSections(
  workspaces: Workspace[],
  folders: Folder[],
): FolderSection[] {
  const known = new Set(folders.map((f) => f.id));
  const members = (id: string | null) =>
    workspaces.filter((w) =>
      id === null ? w.folder === null || !known.has(w.folder) : w.folder === id,
    );
  return [
    { folder: null, rows: workspaceTree(members(null)) },
    ...folders.map((folder) => ({
      folder,
      rows: workspaceTree(members(folder.id)),
    })),
  ];
}

/**
 * The workspaces that may legally become `workspaceId`'s blocker: everything
 * in the same folder except itself and anything already waiting on it, directly
 * or through a chain. Picking one of those would close a cycle.
 *
 * Same folder, because nesting is only ever drawn within one — a cross-folder
 * link would be stored and then never appear. The Rust command enforces both
 * rules too; this exists so an illegal choice is never offered, not to be the
 * only guard. A cycle already present in `state.json` bounds the walk and
 * excludes the candidate.
 */
export function blockerCandidates(
  workspaces: Workspace[],
  workspaceId: WorkspaceId,
): Workspace[] {
  const byId = new Map(workspaces.map((w) => [w.id, w]));
  const folder = byId.get(workspaceId)?.folder ?? null;
  const waitsOnTarget = (start: WorkspaceId): boolean => {
    let cursor: WorkspaceId | null = start;
    for (let hops = 0; hops <= workspaces.length; hops++) {
      if (!cursor) return false;
      if (cursor === workspaceId) return true;
      cursor = byId.get(cursor)?.blocked_by ?? null;
    }
    return true;
  };
  return workspaces.filter(
    (w) =>
      w.id !== workspaceId && w.folder === folder && !waitsOnTarget(w.id),
  );
}
