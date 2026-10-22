import type {
  Folder,
  GithubPrStatus,
  RepoLink,
  Workspace,
  WorkspaceId,
} from "./types";

/** Several missed 45s poller ticks: polling is wedged or backed off. */
const STALE_MS = 5 * 60 * 1000;

export function isStale(fetchedAt: string): boolean {
  const t = new Date(fetchedAt).getTime();
  if (Number.isNaN(t)) return false;
  return Date.now() - t > STALE_MS;
}

export type LinkPr = {
  status: GithubPrStatus;
  number: number;
};

export function linkPrEntries(link: RepoLink): LinkPr[] {
  const out: LinkPr[] = [];
  for (const pr of link.prs) {
    if (pr.status) out.push({ status: pr.status, number: pr.number });
  }
  return out;
}

export type PrGroup = {
  stack: { number: number; size: number } | null;
  /** Base-first. */
  prs: LinkPr[];
};

/** A group can be smaller than `stack.size`: only part may be tracked here. */
export function prGroups(entries: LinkPr[]): PrGroup[] {
  const groups: PrGroup[] = [];
  // A group lands where its first member appeared, so chips don't jump when
  // an unrelated PR is attached.
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
    group.prs.sort((a, b) => a.status.stack!.position - b.status.stack!.position);
  }

  return groups;
}

export type WorkspaceRow = { workspace: Workspace; depth: number };

/** Blocked only if the blocker is among `workspaces`, so deleting or moving a
 *  blocker un-nests its dependents without touching `blocked_by`. Hence one
 *  call per folder. */
function workspaceTree(workspaces: Workspace[]): WorkspaceRow[] {
  const present = new Set(workspaces.map((w) => w.id));
  const childrenOf = new Map<string, Workspace[]>();
  const roots: Workspace[] = [];

  for (const w of workspaces) {
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

  // Unvisited means a cycle no root reaches; show those flat rather than drop them.
  for (const w of workspaces) if (!seen.has(w.id)) walk(w, 0);

  return out;
}

type FolderSection = { folder: Folder | null; rows: WorkspaceRow[] };

/** Empty sections are kept: an empty folder still needs a header to drop onto. */
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

/** Same folder only, since nesting is only drawn within one. Bounded because
 *  `state.json` may already hold a cycle. */
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
