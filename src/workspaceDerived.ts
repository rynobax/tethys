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
