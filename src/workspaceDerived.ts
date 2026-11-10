import type {
  Folder,
  GithubPrStatus,
  RepoLink,
  Workspace,
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

/** Nested only if the parent is among `workspaces`, so a child whose parent
 *  is soft-deleted or gone draws at top level without touching `parent`.
 *  Children sit oldest first, the order they were handed out in. */
function workspaceTree(workspaces: Workspace[]): WorkspaceRow[] {
  const present = new Set(workspaces.map((w) => w.id));
  const childrenOf = new Map<string, Workspace[]>();
  const roots: Workspace[] = [];

  for (const w of workspaces) {
    const parent =
      w.parent && w.parent !== w.id && present.has(w.parent) ? w.parent : null;
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
    const children = [...(childrenOf.get(w.id) ?? [])].sort((a, b) =>
      a.created_at.localeCompare(b.created_at),
    );
    for (const child of children) walk(child, depth + 1);
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

/** Worst first: a tile shows the worst of its open Shipping PRs. */
export type ShippingSignal =
  | "conflict"
  | "ci_failing"
  | "unknown"
  | "review"
  | "ci_pending"
  | "merge_queue"
  | "ready";

const SIGNAL_RANK: Record<ShippingSignal, number> = {
  conflict: 6,
  ci_failing: 5,
  unknown: 4,
  review: 3,
  ci_pending: 2,
  merge_queue: 1,
  ready: 0,
};

const SIGNAL_WORDS: Record<ShippingSignal, string> = {
  conflict: "merge conflicts",
  ci_failing: "CI failing",
  unknown: "no status yet",
  review: "waiting for review",
  ci_pending: "CI running",
  merge_queue: "in the merge queue",
  ready: "ready to merge",
};

function prSignal(status: GithubPrStatus | null): ShippingSignal {
  if (!status) return "unknown";
  if (status.has_merge_conflicts) return "conflict";
  if (status.checks === "failure" || status.merge_queue === "unmergeable") {
    return "ci_failing";
  }
  if (status.merge_queue) return "merge_queue";
  if (status.review_decision !== "approved") return "review";
  if (status.checks === "pending") return "ci_pending";
  return "ready";
}

export type ShippingPr = {
  repoKey: string;
  number: number;
  status: GithubPrStatus | null;
  signal: ShippingSignal;
};

export function shippingPrs(w: Workspace): ShippingPr[] {
  if (w.shipment?.kind !== "shipping") return [];
  return w.shipment.prs.map(({ repo_key, number }) => {
    const status =
      w.repo_links
        .find((l) => l.repo_key === repo_key)
        ?.prs.find((p) => p.number === number)?.status ?? null;
    return { repoKey: repo_key, number, status, signal: prSignal(status) };
  });
}

/** Closed PRs are done and say nothing about what's left. */
export function shippingSignal(prs: ShippingPr[]): ShippingSignal {
  return prs
    .filter((p) => p.status?.state !== "merged" && p.status?.state !== "closed")
    .reduce<ShippingSignal>(
      (worst, p) =>
        SIGNAL_RANK[p.signal] > SIGNAL_RANK[worst] ? p.signal : worst,
      "ready",
    );
}

export function shippingSummary(w: Workspace, prs: ShippingPr[]): string {
  const lines = prs.map((p) => {
    const closed = p.status?.state === "merged" || p.status?.state === "closed";
    const words = closed ? p.status!.state : SIGNAL_WORDS[p.signal];
    const changes =
      !closed && p.status?.review_decision === "changes_requested"
        ? " (changes requested)"
        : "";
    return `${p.repoKey}#${p.number}: ${words}${changes}`;
  });
  return [w.branch, ...lines].join("\n");
}

/** Open PRs a workspace could ship, in tracking order. */
export function shippablePrs(w: Workspace): { repoKey: string; status: GithubPrStatus }[] {
  return w.repo_links.flatMap((link) =>
    link.prs.flatMap((p) =>
      p.status?.state === "open" ? [{ repoKey: link.repo_key, status: p.status }] : [],
    ),
  );
}
