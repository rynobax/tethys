import type { SessionInfo, Workspace, WorkspaceId } from "../types";
import { shippingPrs, shippingSignal } from "../workspaceDerived";
import { ANIMAL_NAMES, type AnimalName } from "./animals";
import { hashString, workFor, type Work } from "./spec";

/** What a resident is up to, read off Tethys state. Renderer-agnostic. */
export type Mood =
  | "queued"
  | "hatching"
  | "failed"
  | "needs_you"
  | "working"
  | "asleep"
  | "idle"
  /** Shipping and waiting on other people: pitching to the crowd. */
  | "shipping"
  | "shipped";

/** One workspace as the world sees it. */
export interface Resident {
  id: WorkspaceId;
  branch: string;
  mood: Mood;
  parentId: WorkspaceId | null;
  /** 0 for a top-level workspace; children are drawn smaller per level. */
  depth: number;
  selected: boolean;
  /** Its family's animal, shared with its parent and children. */
  animal: AnimalName;
  work: Work;
  progress: { done: number; total: number } | null;
  /** A few words beside the branch, e.g. the PRs being pitched. */
  note: string | null;
  /** Shipping, and every PR approved and green: only the merge to go. */
  readyToMerge: boolean;
}

export interface ResidentInputs {
  workspaces: Workspace[];
  sessions: Map<WorkspaceId, SessionInfo | null>;
  needsTurn: (w: Workspace) => boolean;
  working: (w: Workspace) => boolean;
  selectedId: WorkspaceId | null;
  animals: ReadonlyMap<WorkspaceId, AnimalName>;
}

function moodOf(w: Workspace, inputs: ResidentInputs): Mood {
  switch (w.status.kind) {
    case "queued":
      return "queued";
    case "creating":
      return "hatching";
    case "creation_failed":
      return "failed";
  }
  // The tile, not the turn, says when a Shipping workspace wants Ryan: its
  // turns are mostly ones Tethys started.
  const signal = shipSignal(w);
  if (signal) {
    if (signal === "ready" || signal === "no_reviewer") return "needs_you";
    return inputs.working(w) ? "working" : "shipping";
  }
  // Merged, all that's left is to delete it, even with a turn unread.
  if (w.shipment?.kind === "shipped") return "shipped";
  if (inputs.needsTurn(w)) return "needs_you";
  if (inputs.working(w)) return "working";
  const session = inputs.sessions.get(w.id);
  if (!session?.running) return "asleep";
  return "idle";
}

function shipSignal(w: Workspace) {
  return w.shipment?.kind === "shipping"
    ? shippingSignal(shippingPrs(w))
    : null;
}

function noteOf(w: Workspace): string | null {
  const signal = shipSignal(w);
  if (signal === "ready") return "ready to merge";
  if (signal === "no_reviewer") return "no reviewer";
  if (!signal) return null;
  const open = shippingPrs(w).filter(
    (p) => p.status?.state !== "merged" && p.status?.state !== "closed",
  );
  return open.map((p) => `#${p.number}`).join(" ") || null;
}

export function toResidents(inputs: ResidentInputs): Resident[] {
  const live = inputs.workspaces.filter((w) => !w.deleted_at);
  const byId = new Map(live.map((w) => [w.id, w]));
  const depthOf = (w: Workspace): number => {
    let depth = 0;
    let cur = w.parent ? byId.get(w.parent) : undefined;
    while (cur && depth < 8) {
      depth++;
      cur = cur.parent ? byId.get(cur.parent) : undefined;
    }
    return depth;
  };
  return live.map((w) => {
    return {
      id: w.id,
      branch: w.branch,
      mood: moodOf(w, inputs),
      parentId: w.parent && byId.has(w.parent) ? w.parent : null,
      depth: depthOf(w),
      selected: w.id === inputs.selectedId,
      animal:
        inputs.animals.get(w.id) ??
        ANIMAL_NAMES[hashString(w.id) % ANIMAL_NAMES.length],
      work: workFor(w.id),
      progress: w.progress,
      note: noteOf(w),
      readyToMerge: shipSignal(w) === "ready",
    };
  });
}
