/**
 * Where everyone is and what they're doing, on flat ground. Knows nothing
 * about how it's drawn: a 3D scene and an isometric one would both read
 * `agents` and draw them their own way.
 *
 * The ground is split by the terminal into two meadows joined by a portal
 * each, so a resident wandering off one side can come out the other.
 */

import type { Mood, Resident } from "./residents";
import { hashString } from "./spec";

export type Side = "left" | "right";

export interface Vec2 {
  x: number;
  z: number;
}

/** Walkable ground; +z is toward the viewer. */
export interface Region {
  xMin: number;
  xMax: number;
  zMin: number;
  zMax: number;
}

export interface Layout {
  /** `null` when the window leaves no room on that side. */
  regions: Record<Side, Region | null>;
  portals: Record<Side, Vec2 | null>;
}

export type Activity =
  | "walk"
  | "pause"
  | "work"
  | "attention"
  | "sleep"
  | "egg"
  | "portal_out"
  | "portal_in";

type Intent = "wander" | "portal" | "work" | "attention" | "follow";

export interface Agent {
  id: string;
  /** False when it arrived before there was any ground to put it on. */
  placed: boolean;
  side: Side;
  pos: Vec2;
  /** Radians about y; 0 faces the viewer. */
  facing: number;
  activity: Activity;
  /** Sim time the activity began, for animation phase. */
  since: number;
  /** 0..1, shrinking into a portal and growing out of the other. */
  portalScale: number;
  /** Per-agent offset so nobody animates in lockstep. */
  phase: number;
  /** Drawn size, 1 for a top-level workspace. */
  size: number;
  target: Vec2 | null;
  intent: Intent | null;
  until: number;
  workSpot: Vec2 | null;
}

const PORTAL_SECONDS = 0.6;
const ARRIVE = 0.12;
const PORTAL_CHANCE = 0.12;
const FOLLOW_CHANCE = 0.75;
const WANDER_RADIUS = 4;
const ATTENTION_SPACING = 1.5;

const rand = (a: number, b: number) => a + Math.random() * (b - a);
const dist = (a: Vec2, b: Vec2) => Math.hypot(a.x - b.x, a.z - b.z);
const other = (s: Side): Side => (s === "left" ? "right" : "left");

export const sizeForDepth = (depth: number) => Math.max(0.45, 0.65 ** depth);

/** Moods that walk about freely when nothing else is asked of them. */
const ROAMING: ReadonlySet<Mood> = new Set(["idle", "shipping", "shipped"]);

export class Sim {
  readonly agents = new Map<string, Agent>();
  private residents = new Map<string, Resident>();
  private layout: Layout | null = null;
  private now = 0;

  setLayout(layout: Layout) {
    this.layout = layout;
    for (const a of this.agents.values()) this.settle(a);
  }

  sync(residents: Resident[]) {
    this.residents = new Map(residents.map((r) => [r.id, r]));
    for (const id of this.agents.keys()) {
      if (!this.residents.has(id)) this.agents.delete(id);
    }
    // Parents first, so a new child can spawn beside its parent.
    const ordered = [...residents].sort((a, b) => a.depth - b.depth);
    for (const r of ordered) {
      const existing = this.agents.get(r.id);
      if (existing) {
        existing.size = sizeForDepth(r.depth);
      } else {
        this.agents.set(r.id, this.spawn(r));
      }
    }
  }

  step(dt: number) {
    if (!this.layout) return;
    this.now += dt;
    for (const a of this.agents.values()) {
      const r = this.residents.get(a.id);
      if (!r) continue;
      this.think(a, r);
      this.move(a, dt, r.mood === "needs_you" ? 2.2 : 1.2 + (1 - a.size));
    }
    this.separate();
  }

  time() {
    return this.now;
  }

  private region(side: Side): Region | null {
    return this.layout?.regions[side] ?? null;
  }

  private randomPoint(
    region: Region,
    near?: Vec2,
    radius = WANDER_RADIUS,
  ): Vec2 {
    // Clamped, or a point near someone outside the meadow lands outside it.
    const cx = clamp(
      near?.x ?? (region.xMin + region.xMax) / 2,
      region.xMin,
      region.xMax,
    );
    const cz = clamp(
      near?.z ?? (region.zMin + region.zMax) / 2,
      region.zMin,
      region.zMax,
    );
    const r = near ? radius : Infinity;
    return {
      x: rand(Math.max(region.xMin, cx - r), Math.min(region.xMax, cx + r)),
      z: rand(Math.max(region.zMin, cz - r), Math.min(region.zMax, cz + r)),
    };
  }

  private spawn(r: Resident): Agent {
    const parent = r.parentId ? this.agents.get(r.parentId) : undefined;
    let side: Side = parent?.side ?? (hashString(r.id) % 2 ? "left" : "right");
    if (!this.region(side) && this.region(other(side))) side = other(side);
    const region = this.region(side);
    const pos = region
      ? this.randomPoint(region, parent?.pos, 1.5)
      : { x: 0, z: 0 };
    return {
      id: r.id,
      placed: region !== null,
      side,
      pos,
      facing: rand(-0.6, 0.6),
      activity: "pause",
      since: this.now,
      portalScale: 1,
      phase: rand(0, Math.PI * 2),
      size: sizeForDepth(r.depth),
      target: null,
      intent: null,
      until: this.now + rand(0.2, 2),
      workSpot: null,
    };
  }

  /** After a resize: back inside the meadow, or over to the other one. */
  private settle(a: Agent) {
    if (!a.placed && this.region(a.side)) {
      const parent = this.parentOf(a);
      a.pos = this.randomPoint(
        this.region(a.side)!,
        parent?.placed ? parent.pos : undefined,
        1.5,
      );
      a.placed = true;
    }
    if (!this.region(a.side) && this.region(other(a.side))) {
      a.side = other(a.side);
      a.pos = this.randomPoint(this.region(a.side)!);
      a.target = null;
      a.workSpot = null;
    }
    const region = this.region(a.side);
    if (!region) return;
    a.pos.x = clamp(a.pos.x, region.xMin, region.xMax);
    a.pos.z = clamp(a.pos.z, region.zMin, region.zMax);
    if (a.workSpot && !inside(region, a.workSpot)) a.workSpot = null;
  }

  private parentOf(a: Agent): Agent | undefined {
    const parentId = this.residents.get(a.id)?.parentId;
    return parentId ? this.agents.get(parentId) : undefined;
  }

  private setActivity(a: Agent, activity: Activity) {
    if (a.activity === activity) return;
    a.activity = activity;
    a.since = this.now;
  }

  private walkTo(a: Agent, target: Vec2, intent: Intent) {
    a.target = target;
    a.intent = intent;
    this.setActivity(a, "walk");
  }

  private pause(a: Agent, min: number, max: number) {
    a.target = null;
    a.intent = null;
    a.until = this.now + rand(min, max);
    this.setActivity(a, "pause");
  }

  private think(a: Agent, r: Resident) {
    if (a.activity === "portal_out") {
      if (this.now - a.since < PORTAL_SECONDS) return;
      const to = other(a.side);
      const portal = this.layout?.portals[to];
      if (!portal) {
        this.setActivity(a, "portal_in");
        return;
      }
      a.side = to;
      a.pos = { x: portal.x, z: portal.z + 0.6 };
      a.workSpot = null;
      this.setActivity(a, "portal_in");
      return;
    }
    if (a.activity === "portal_in") {
      if (this.now - a.since >= PORTAL_SECONDS) this.pause(a, 0.3, 1.2);
      return;
    }

    const region = this.region(a.side);
    if (!region) return;

    switch (r.mood) {
      case "queued":
      case "hatching":
      case "failed":
        a.target = null;
        this.setActivity(a, "egg");
        return;
      case "asleep":
        a.target = null;
        this.setActivity(a, "sleep");
        return;
      case "needs_you":
        this.head(a, this.attentionSpot(a, region), "attention");
        return;
      case "working":
        if (!a.workSpot) a.workSpot = this.randomPoint(region, a.pos, 3);
        this.head(a, a.workSpot, "work");
        return;
      default:
        this.roam(a, r, region);
    }
  }

  /** Walk to a spot and stay there doing `intent`. */
  private head(a: Agent, spot: Vec2, intent: "attention" | "work") {
    const arrived = dist(a.pos, spot) < (a.activity === intent ? 0.5 : ARRIVE);
    if (arrived) {
      a.target = null;
      this.setActivity(a, intent);
    } else {
      this.walkTo(a, spot, intent);
    }
  }

  /** Lined up at the edge nearest the terminal, as close as it can get. */
  private attentionSpot(a: Agent, region: Region): Vec2 {
    const waiting = [...this.agents.values()]
      .filter(
        (o) =>
          o.side === a.side && this.residents.get(o.id)?.mood === "needs_you",
      )
      .map((o) => o.id)
      .sort();
    const i = waiting.indexOf(a.id);
    const n = waiting.length;
    const mid = (region.zMin + region.zMax) / 2;
    const z = mid + (i - (n - 1) / 2) * ATTENTION_SPACING;
    return {
      x: a.side === "left" ? region.xMax - 0.4 : region.xMin + 0.4,
      z: clamp(z, region.zMin, region.zMax),
    };
  }

  private roam(a: Agent, r: Resident, region: Region) {
    a.workSpot = null;
    const parent = r.parentId ? this.agents.get(r.parentId) : undefined;
    const parentRoams =
      parent && ROAMING.has(this.residents.get(parent.id)?.mood ?? "asleep");

    if (a.activity === "walk") {
      if (a.intent === "follow" && parent) {
        if (parent.side !== a.side) {
          this.toPortal(a);
          return;
        }
        a.target = this.behind(parent, a);
      }
      if (a.target && dist(a.pos, a.target) < ARRIVE) {
        if (a.intent === "portal") {
          this.setActivity(a, "portal_out");
        } else if (a.intent === "follow") {
          this.pause(a, 0.2, 0.7);
        } else {
          this.pause(a, 1.5, 5);
        }
      }
      return;
    }
    if (a.activity === "pause" && this.now < a.until) return;
    if (a.activity !== "pause") {
      // Just finished working, waking, hatching or being needed.
      this.pause(a, 0.5, 2);
      return;
    }

    if (parent && parentRoams && Math.random() < FOLLOW_CHANCE) {
      if (parent.side !== a.side) this.toPortal(a);
      else this.walkTo(a, this.behind(parent, a), "follow");
    } else if (
      Math.random() < PORTAL_CHANCE &&
      this.layout?.portals[other(a.side)]
    ) {
      this.toPortal(a);
    } else {
      this.walkTo(a, this.randomPoint(region, a.pos), "wander");
    }
  }

  private toPortal(a: Agent) {
    const portal = this.layout?.portals[a.side];
    if (portal && this.layout?.portals[other(a.side)]) {
      this.walkTo(a, { ...portal }, "portal");
    } else {
      this.pause(a, 1, 3);
    }
  }

  /** Ducklings: a trailing spot behind the parent, staggered per child. */
  private behind(parent: Agent, child: Agent): Vec2 {
    const siblings = [...this.agents.values()]
      .filter((o) => this.residents.get(o.id)?.parentId === parent.id)
      .map((o) => o.id)
      .sort();
    const i = Math.max(0, siblings.indexOf(child.id));
    const back = 0.8 + i * 0.6;
    const spread = (i % 2 ? 1 : -1) * 0.25;
    return {
      x: parent.pos.x - Math.sin(parent.facing) * back + spread,
      z: parent.pos.z - Math.cos(parent.facing) * back,
    };
  }

  private move(a: Agent, dt: number, speed: number) {
    if (a.activity === "portal_out") {
      a.portalScale = Math.max(0, 1 - (this.now - a.since) / PORTAL_SECONDS);
      return;
    }
    if (a.activity === "portal_in") {
      a.portalScale = Math.min(1, (this.now - a.since) / PORTAL_SECONDS);
      return;
    }
    a.portalScale = 1;

    let wantFacing: number | null = null;
    if (a.activity === "walk" && a.target) {
      const dx = a.target.x - a.pos.x;
      const dz = a.target.z - a.pos.z;
      const d = Math.hypot(dx, dz);
      if (d > 1e-4) {
        const stepLen = Math.min(d, speed * dt);
        a.pos.x += (dx / d) * stepLen;
        a.pos.z += (dz / d) * stepLen;
        wantFacing = Math.atan2(dx, dz);
      }
    } else if (a.activity === "attention" || a.activity === "work") {
      wantFacing = a.activity === "work" ? 0.35 * Math.sin(a.phase) : 0;
    }
    if (wantFacing !== null) {
      let diff = wantFacing - a.facing;
      while (diff > Math.PI) diff -= Math.PI * 2;
      while (diff < -Math.PI) diff += Math.PI * 2;
      a.facing += diff * Math.min(1, dt * 8);
    }
  }

  /** Nobody stands inside anybody else. Only the ones moving get pushed. */
  private separate() {
    const list = [...this.agents.values()];
    const movable = (a: Agent) =>
      a.activity === "walk" || a.activity === "pause";
    for (let i = 0; i < list.length; i++) {
      for (let j = i + 1; j < list.length; j++) {
        const a = list[i];
        const b = list[j];
        if (a.side !== b.side) continue;
        const min = (a.size + b.size) * 0.4;
        const dx = b.pos.x - a.pos.x;
        const dz = b.pos.z - a.pos.z;
        const d = Math.hypot(dx, dz);
        if (d >= min || d < 1e-5) continue;
        const push = (min - d) / 2;
        const ux = dx / d;
        const uz = dz / d;
        if (movable(a)) {
          a.pos.x -= ux * push;
          a.pos.z -= uz * push;
        }
        if (movable(b)) {
          b.pos.x += ux * push;
          b.pos.z += uz * push;
        }
      }
    }
  }
}

function clamp(n: number, min: number, max: number) {
  return Math.min(max, Math.max(min, n));
}

function inside(r: Region, p: Vec2) {
  return p.x >= r.xMin && p.x <= r.xMax && p.z >= r.zMin && p.z <= r.zMax;
}
