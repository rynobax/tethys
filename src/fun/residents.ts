import type { SessionInfo, Workspace, WorkspaceId } from "../types";
import { defaultSpec, normalizeSpec, type AvatarSpec } from "./spec";

/** What a resident is up to, read off Tethys state. Renderer-agnostic. */
export type Mood =
  | "queued"
  | "hatching"
  | "failed"
  | "needs_you"
  | "working"
  | "asleep"
  | "idle"
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
  spec: AvatarSpec;
  /** Whether `spec` came from a design rather than the default bean. */
  designed: boolean;
  progress: { done: number; total: number } | null;
}

export interface ResidentInputs {
  workspaces: Workspace[];
  sessions: Map<WorkspaceId, SessionInfo | null>;
  needsTurn: (w: Workspace) => boolean;
  working: (w: Workspace) => boolean;
  selectedId: WorkspaceId | null;
  avatars: Map<WorkspaceId, unknown>;
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
  if (inputs.needsTurn(w)) return "needs_you";
  if (w.shipment?.kind === "shipped") return "shipped";
  if (inputs.working(w)) return "working";
  const session = inputs.sessions.get(w.id);
  if (!session?.running) return "asleep";
  if (w.shipment?.kind === "shipping") return "shipping";
  return "idle";
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
    const fallback = defaultSpec(w.id, w.branch);
    const raw = inputs.avatars.get(w.id);
    return {
      id: w.id,
      branch: w.branch,
      mood: moodOf(w, inputs),
      parentId: w.parent && byId.has(w.parent) ? w.parent : null,
      depth: depthOf(w),
      selected: w.id === inputs.selectedId,
      spec: raw ? normalizeSpec(raw, fallback) : fallback,
      designed: raw !== undefined,
      progress: w.progress,
    };
  });
}
