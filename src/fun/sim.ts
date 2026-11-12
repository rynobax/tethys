/**
 * Where everyone is and what they're doing, on flat ground. Knows nothing
 * about how it's drawn: a 3D scene and an isometric one would both read
 * `agents` and draw them their own way.
 *
 * The ground is split by the terminal into two meadows joined by a portal
 * each. The left is for workspaces that need you, each in a spot of its own;
 * everyone else lives on the right. Crossing over is always by portal.
 */

import type { Mood, Resident } from "./residents";

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
  /** Where waiting residents stand, best first; far enough apart for labels. */
  attentionSlots: Vec2[];
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
  /** Where it works, or waits when there's no slot to wait in. */
  spot: Vec2 | null;
  /** Closest it has got to `target`, and when, to notice being stuck. */
  bestDist: number;
  bestAt: number;
}

const PORTAL_SECONDS = 0.6;
const ARRIVE = 0.2;
const FOLLOW_ARRIVE = 0.35;
/** No closer to the target in this long and it gives up. */
const STUCK_SECONDS = 1.2;
const FOLLOW_CHANCE = 0.75;
const WANDER_RADIUS = 4;
const CANDIDATES = 8;

const SPEED_STROLL = 1.2;
const SPEED_HURRY = 4.5;

const rand = (a: number, b: number) => a + Math.random() * (b - a);
const dist = (a: Vec2, b: Vec2) => Math.hypot(a.x - b.x, a.z - b.z);
const other = (s: Side): Side => (s === "left" ? "right" : "left");

export const sizeForDepth = (depth: number) => Math.max(0.45, 0.65 ** depth);

/** Moods that walk about freely when nothing else is asked of them. */
const ROAMING: ReadonlySet<Mood> = new Set(["idle", "shipping", "shipped"]);
const EGG: ReadonlySet<Mood> = new Set(["queued", "hatching", "failed"]);

export class Sim {
  readonly agents = new Map<string, Agent>();
  private residents = new Map<string, Resident>();
  private layout: Layout | null = null;
  /** Attention slot index per waiting agent, kept until it stops waiting. */
  private slots = new Map<string, number>();
  private now = 0;

  setLayout(layout: Layout) {
    this.layout = layout;
    for (const [id, i] of this.slots) {
      if (i >= layout.attentionSlots.length) this.slots.delete(id);
    }
    for (const a of this.agents.values()) this.settle(a);
  }

  sync(residents: Resident[]) {
    this.residents = new Map(residents.map((r) => [r.id, r]));
    for (const id of this.agents.keys()) {
      if (!this.residents.has(id)) {
        this.agents.delete(id);
        this.slots.delete(id);
      }
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
      const hurry = r.mood === "needs_you" || a.intent === "portal";
      this.move(a, dt, hurry ? SPEED_HURRY : SPEED_STROLL + (1 - a.size));
    }
    this.separate();
  }

  time() {
    return this.now;
  }

  private region(side: Side): Region | null {
    return this.layout?.regions[side] ?? null;
  }

  /** Waiting on the left, everyone else on the right, if there's room. */
  private homeSide(mood: Mood): Side {
    const want: Side = mood === "needs_you" ? "left" : "right";
    return this.region(want) || !this.region(other(want)) ? want : other(want);
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

  /** Where someone on this side is, or is about to be. */
  private claim(o: Agent): Vec2 {
    return o.spot ?? o.target ?? o.pos;
  }

  /** The best of a few random points: the one furthest from everyone else. */
  private spacedPoint(
    self: string,
    side: Side,
    region: Region,
    near?: Vec2,
    radius?: number,
  ): Vec2 {
    const portal = this.layout?.portals[side];
    const others = [...this.agents.values()]
      .filter((o) => o.id !== self && o.side === side)
      .map((o) => this.claim(o));
    // The portal's doorway counts as taken, so nobody sets up in it.
    if (portal) others.push(portal, { x: portal.x, z: portal.z + 1.2 });
    let best = this.randomPoint(region, near, radius);
    let bestScore = -1;
    for (let i = 0; i < CANDIDATES; i++) {
      const p = i === 0 ? best : this.randomPoint(region, near, radius);
      const score = Math.min(...others.map((o) => dist(o, p)));
      if (score > bestScore) {
        bestScore = score;
        best = p;
      }
    }
    return best;
  }

  private spawn(r: Resident): Agent {
    const a: Agent = {
      id: r.id,
      placed: false,
      side: this.homeSide(r.mood),
      pos: { x: 0, z: 0 },
      facing: rand(-0.6, 0.6),
      activity: "pause",
      since: this.now,
      portalScale: 1,
      phase: rand(0, Math.PI * 2),
      size: sizeForDepth(r.depth),
      target: null,
      intent: null,
      until: this.now + rand(0.2, 2),
      spot: null,
      bestDist: Infinity,
      bestAt: this.now,
    };
    this.place(a, r);
    return a;
  }

  /** Straight onto its home ground: no walking in on first sight. */
  private place(a: Agent, r: Resident) {
    a.side = this.homeSide(r.mood);
    const region = this.region(a.side);
    if (!region) return;
    const slot = r.mood === "needs_you" ? this.attentionSpot(a) : null;
    const parent = this.parentOf(a);
    a.pos = slot
      ? { ...slot }
      : this.spacedPoint(
          a.id,
          a.side,
          region,
          parent?.placed && parent.side === a.side ? parent.pos : undefined,
          1.5,
        );
    a.placed = true;
  }

  /** After a resize: back inside the meadow, or over to the other one. */
  private settle(a: Agent) {
    const r = this.residents.get(a.id);
    if (!r) return;
    if (!a.placed || !this.region(a.side)) {
      a.target = null;
      a.spot = null;
      this.place(a, r);
    }
    const region = this.region(a.side);
    if (!region) return;
    a.pos.x = clamp(a.pos.x, region.xMin, region.xMax);
    a.pos.z = clamp(a.pos.z, region.zMin, region.zMax);
    if (a.spot && !inside(region, a.spot)) a.spot = null;
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

  /** Re-aiming at about the same place keeps the stuck clock running. */
  private walkTo(a: Agent, target: Vec2, intent: Intent) {
    const same =
      a.activity === "walk" &&
      a.intent === intent &&
      a.target !== null &&
      dist(a.target, target) < 0.3;
    if (!same) {
      a.bestDist = Infinity;
      a.bestAt = this.now;
    }
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

  private stuck(a: Agent) {
    return a.activity === "walk" && this.now - a.bestAt > STUCK_SECONDS;
  }

  private think(a: Agent, r: Resident) {
    if (a.activity === "portal_out") {
      if (this.now - a.since < PORTAL_SECONDS) return;
      const to = other(a.side);
      const portal = this.layout?.portals[to];
      a.side = to;
      a.pos = portal ? { x: portal.x, z: portal.z + 0.6 } : a.pos;
      a.spot = null;
      a.target = null;
      a.intent = null;
      this.setActivity(a, "portal_in");
      return;
    }
    if (a.activity === "portal_in") {
      if (this.now - a.since >= PORTAL_SECONDS) this.pause(a, 0.2, 0.6);
      return;
    }

    if (r.mood !== "needs_you" || a.side !== "left") this.slots.delete(a.id);

    if (EGG.has(r.mood)) {
      a.target = null;
      this.setActivity(a, "egg");
      return;
    }
    if (a.side !== this.homeSide(r.mood)) {
      this.toPortal(a);
      return;
    }
    const region = this.region(a.side);
    if (!region) return;

    switch (r.mood) {
      case "asleep":
        a.target = null;
        a.spot = null;
        this.setActivity(a, "sleep");
        return;
      case "needs_you": {
        const slot = this.attentionSpot(a);
        if (!slot && !a.spot) {
          a.spot = this.spacedPoint(a.id, a.side, region);
        }
        this.head(a, slot ?? a.spot!, "attention");
        return;
      }
      case "working":
        if (!a.spot || this.stuck(a)) {
          a.spot = this.spacedPoint(a.id, a.side, region, a.pos, 3);
        }
        this.head(a, a.spot, "work");
        return;
      default:
        a.spot = null;
        this.roam(a, r, region);
    }
  }

  /** Walk to a spot and stay there doing `intent`. */
  private head(a: Agent, spot: Vec2, intent: "attention" | "work") {
    const reach = a.activity === intent ? 0.5 : ARRIVE;
    // Close enough and blocked counts as there, rather than shoving forever.
    if (dist(a.pos, spot) < reach || (this.stuck(a) && dist(a.pos, spot) < 1)) {
      a.target = null;
      this.setActivity(a, intent);
    } else {
      this.walkTo(a, spot, intent);
    }
  }

  /** This agent's slot on the left, claiming the best free one if it has none. */
  private attentionSpot(a: Agent): Vec2 | null {
    const slots = this.layout?.attentionSlots ?? [];
    if (a.side !== "left" || !slots.length) return null;
    let i = this.slots.get(a.id);
    if (i === undefined) {
      const taken = new Set(this.slots.values());
      i = slots.findIndex((_, j) => !taken.has(j));
      if (i < 0) return null;
      this.slots.set(a.id, i);
    }
    return slots[i];
  }

  private roam(a: Agent, r: Resident, region: Region) {
    const parent = r.parentId ? this.agents.get(r.parentId) : undefined;
    const parentRoams =
      parent &&
      parent.side === a.side &&
      ROAMING.has(this.residents.get(parent.id)?.mood ?? "asleep");

    if (a.activity === "walk") {
      if (a.intent === "follow") {
        if (!parentRoams) {
          this.pause(a, 0.5, 1.5);
          return;
        }
        a.target = this.behind(parent, a);
      }
      const reach = a.intent === "follow" ? FOLLOW_ARRIVE : ARRIVE;
      if (a.target && dist(a.pos, a.target) < reach) {
        if (a.intent === "follow") this.pause(a, 0.2, 0.7);
        else this.pause(a, 1.5, 5);
      } else if (this.stuck(a)) {
        this.pause(a, 0.3, 1.2);
      }
      return;
    }
    if (a.activity === "pause" && this.now < a.until) return;
    if (a.activity !== "pause") {
      // Just finished working, waking, hatching or being needed.
      this.pause(a, 0.5, 2);
      return;
    }

    if (parentRoams && Math.random() < FOLLOW_CHANCE) {
      this.walkTo(a, this.behind(parent, a), "follow");
    } else {
      this.walkTo(a, this.spacedPoint(a.id, a.side, region, a.pos), "wander");
    }
  }

  private toPortal(a: Agent) {
    const portal = this.layout?.portals[a.side];
    if (!portal || !this.layout?.portals[other(a.side)]) return;
    if (a.activity === "walk" && a.intent === "portal") {
      // A crowd at the portal mustn't hold anyone back from it.
      if (dist(a.pos, portal) < ARRIVE || this.stuck(a)) {
        this.setActivity(a, "portal_out");
      }
      return;
    }
    if (a.activity === "portal_out") return;
    this.walkTo(a, { ...portal }, "portal");
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
      if (d < a.bestDist - 0.05) {
        a.bestDist = d;
        a.bestAt = this.now;
      }
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

  /**
   * Nobody stands inside anybody else. Whoever is settled holds its ground
   * and the one moving steps round it; two settled ones don't budge.
   */
  private separate() {
    const list = [...this.agents.values()];
    const movable = (a: Agent) =>
      a.activity === "walk" || a.activity === "pause";
    for (let i = 0; i < list.length; i++) {
      for (let j = i + 1; j < list.length; j++) {
        const a = list[i];
        const b = list[j];
        if (a.side !== b.side) continue;
        const ma = movable(a);
        const mb = movable(b);
        if (!ma && !mb) continue;
        const min = (a.size + b.size) * 0.4;
        const dx = b.pos.x - a.pos.x;
        const dz = b.pos.z - a.pos.z;
        const d = Math.hypot(dx, dz);
        if (d >= min || d < 1e-5) continue;
        const push = (min - d) / (ma && mb ? 2 : 1);
        // Sideways as well as apart, so two walking head-on slide past.
        const ux = dx / d - (dz / d) * 0.5;
        const uz = dz / d + (dx / d) * 0.5;
        if (ma) {
          a.pos.x -= ux * push;
          a.pos.z -= uz * push;
        }
        if (mb) {
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
