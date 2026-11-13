/**
 * Where everyone is and what they're doing, on flat ground. Knows nothing
 * about how it's drawn: a 3D scene and an isometric one would both read
 * `agents` and draw them their own way.
 *
 * The ground is split by the terminal into two meadows joined by a portal
 * each. The left is for workspaces that need you, each in a spot of its own.
 * The right is everyone else, in zones by what they're doing: a lounge at the
 * back for the ones you've dismissed, a workshop in the middle for the ones
 * working, and a stage at the front while anything is Shipping. Crossing
 * between meadows is always by portal.
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
  /** The right meadow's zones; `null` when it's too short to divide. */
  lounge: Lounge | null;
  workshop: Workshop | null;
  pitch: Pitch | null;
}

/** Where dismissed residents relax: round a fire, or asleep on a bedroll. */
export interface Lounge {
  area: Region;
  fire: Vec2;
  seats: Vec2[];
  naps: Vec2[];
}

/** Where working residents work, at a task of their own or a shared one. */
export interface Workshop {
  /** Its bottom edge gives way to the stage while anyone is pitching. */
  area: Region;
  /** Where someone stands to write on each whiteboard. */
  boards: Vec2[];
  /** Crates are hauled from the first pile to the second. */
  piles: [Vec2, Vec2];
}

/** Where Shipping residents pitch, and the crowd they pitch to. */
export interface Pitch {
  /** Where the one pitching stands. */
  podium: Vec2;
  /** Where the rest wait their turn, best first. */
  chairs: Vec2[];
  crowd: Vec2[];
  /** The stage's footprint. */
  stage: Region;
  /** Everyone else keeps above this while anyone is pitching. */
  top: number;
}

export type Activity =
  | "walk"
  | "pause"
  | "work"
  | "attention"
  | "sleep"
  | "egg"
  | "portal_out"
  | "portal_in"
  | "pitch"
  | "seated"
  | "lounge"
  | "board";

type Post =
  "attention" | "work" | "pitch" | "seated" | "lounge" | "sleep" | "board";
type Intent = "wander" | "portal" | "follow" | "haul" | Post;

/** What a working resident is busy with, for a while. */
export type Task = "station" | "haul" | "board";

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
  /** A working resident's current task, and when it moves on to another. */
  task: Task | null;
  taskUntil: number;
  /** Hauling: which pile it's heading for, and whether it has a crate. */
  haulLeg: 0 | 1;
  carrying: boolean;
  /** Lounging: by the fire, or up for a stroll, and until when. */
  rest: "seat" | "stroll";
  restUntil: number;
}

const PORTAL_SECONDS = 0.6;
const ARRIVE = 0.2;
const FOLLOW_ARRIVE = 0.35;
/** No closer to the target in this long and it gives up. */
const STUCK_SECONDS = 1.2;
const FOLLOW_CHANCE = 0.75;
/** How far a child can get from the parent holding its leash. */
export const LEASH_LENGTH = 2.2;
/** Children pick spots this far in, so the leash usually hangs slack. */
const LEASH_REACH = LEASH_LENGTH * 0.7;
const WANDER_RADIUS = 4;
const CANDIDATES = 8;

const SPEED_STROLL = 1.2;
const SPEED_HURRY = 4.5;

/** How long each Shipping resident gets at the front before the next. */
const PITCH_TURN_SECONDS = 25;
/** Everyone is drawn this size; a top-level resident is about 0.75 units tall. */
const SCALE = 0.7;

const rand = (a: number, b: number) => a + Math.random() * (b - a);
const dist = (a: Vec2, b: Vec2) => Math.hypot(a.x - b.x, a.z - b.z);
const other = (s: Side): Side => (s === "left" ? "right" : "left");

export const sizeForDepth = (depth: number) =>
  SCALE * Math.max(0.45, 0.65 ** depth);

/** Moods that walk about freely when nothing else is asked of them. */
const ROAMING: ReadonlySet<Mood> = new Set(["idle", "shipped"]);
const EGG: ReadonlySet<Mood> = new Set(["queued", "hatching", "failed"]);
/** Posts with something to sit or lie on, which an arrival snaps onto. */
const FURNISHED: ReadonlySet<Post> = new Set(["sleep", "lounge", "seated"]);
/** The ones Ryan has dismissed, who go to the lounge. */
const RESTING: ReadonlySet<Mood> = new Set(["idle", "shipped", "asleep"]);

export class Sim {
  readonly agents = new Map<string, Agent>();
  private residents = new Map<string, Resident>();
  private layout: Layout | null = null;
  /** Slot index per agent in each shared place, kept until it leaves. */
  private waitSlots = new Map<string, number>();
  private chairs = new Map<string, number>();
  private seats = new Map<string, number>();
  private naps = new Map<string, number>();
  private boards = new Map<string, number>();
  /** Shipping residents in turn order; the first is the one pitching. */
  private queue: string[] = [];
  private speakerSince = 0;
  /** Whether anyone is on stage, so the rest keep off it. */
  private pitching = false;
  private now = 0;

  setLayout(layout: Layout) {
    this.layout = layout;
    prune(this.waitSlots, layout.attentionSlots.length);
    prune(this.chairs, layout.pitch?.chairs.length ?? 0);
    prune(this.seats, layout.lounge?.seats.length ?? 0);
    prune(this.naps, layout.lounge?.naps.length ?? 0);
    prune(this.boards, layout.workshop?.boards.length ?? 0);
    for (const a of this.agents.values()) this.settle(a);
  }

  sync(residents: Resident[]) {
    this.residents = new Map(residents.map((r) => [r.id, r]));
    this.pitching = residents.some((r) => r.mood === "shipping");
    const shipping = new Set(
      residents.filter((r) => r.mood === "shipping").map((r) => r.id),
    );
    const speaker = this.queue[0];
    this.queue = this.queue.filter((id) => shipping.has(id));
    for (const id of [...shipping].sort()) {
      if (!this.queue.includes(id)) this.queue.push(id);
    }
    if (this.queue[0] !== speaker) this.speakerSince = this.now;
    for (const id of this.agents.keys()) {
      if (!this.residents.has(id)) {
        this.agents.delete(id);
        for (const book of this.books()) book.delete(id);
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
    // The speaker takes a seat at the back of the queue and the next gets up.
    if (
      this.queue.length > 1 &&
      this.now - this.speakerSince > PITCH_TURN_SECONDS
    ) {
      this.queue.push(this.queue.shift()!);
      this.speakerSince = this.now;
    }
    for (const a of this.agents.values()) {
      const r = this.residents.get(a.id);
      if (!r) continue;
      this.think(a, r);
      const hurry = r.mood === "needs_you" || a.intent === "portal";
      this.move(a, dt, hurry ? SPEED_HURRY : SPEED_STROLL + (1 - a.size));
    }
    this.separate();
    this.tug();
  }

  time() {
    return this.now;
  }

  /** Whether someone has this stage chair, so it's drawn. */
  chairTaken(index: number): boolean {
    for (const i of this.chairs.values()) if (i === index) return true;
    return false;
  }

  /**
   * Whether a child is on its parent's leash right now: both in the right
   * meadow, both out and about. The left meadow is spots, not walks.
   */
  leashed(childId: string, parentId: string): boolean {
    const child = this.agents.get(childId);
    const parent = this.agents.get(parentId);
    if (!child || !parent || !child.placed || !parent.placed) return false;
    if (this.residents.get(childId)?.parentId !== parentId) return false;
    const free = (a: Agent) =>
      a.side === "right" &&
      a.activity !== "egg" &&
      a.activity !== "portal_out" &&
      a.activity !== "portal_in";
    return free(child) && free(parent);
  }

  private books() {
    return [this.waitSlots, this.chairs, this.seats, this.naps, this.boards];
  }

  /** The parent on the other end of this agent's leash, if it's on one. */
  private holder(a: Agent): Agent | undefined {
    const parent = this.parentOf(a);
    return parent && this.leashed(a.id, parent.id) ? parent : undefined;
  }

  /**
   * A parent that walks off drags its children along, whatever they're doing.
   * Parents first, so a grandchild is pulled after its own parent moved.
   */
  private tug() {
    const byDepth = [...this.agents.values()].sort((a, b) => b.size - a.size);
    for (const child of byDepth) {
      const parent = this.holder(child);
      if (!parent) continue;
      const dx = child.pos.x - parent.pos.x;
      const dz = child.pos.z - parent.pos.z;
      const d = Math.hypot(dx, dz);
      if (d <= LEASH_LENGTH) continue;
      child.pos.x = parent.pos.x + (dx / d) * LEASH_LENGTH;
      child.pos.z = parent.pos.z + (dz / d) * LEASH_LENGTH;
      // Dragged off its station: it sets up again within reach.
      if (child.activity === "work") child.spot = null;
    }
  }

  private region(side: Side): Region | null {
    return this.layout?.regions[side] ?? null;
  }

  /** Waiting on the left, everyone else on the right, if there's room. */
  private homeSide(mood: Mood): Side {
    const want: Side = mood === "needs_you" ? "left" : "right";
    return this.region(want) || !this.region(other(want)) ? want : other(want);
  }

  /** The workshop, giving its bottom edge to the stage while it's in use. */
  private workshopArea(): Region | null {
    const w = this.layout?.workshop;
    const pitch = this.layout?.pitch;
    if (!w) return null;
    if (!pitch || !this.pitching) return w.area;
    return { ...w.area, zMax: Math.max(w.area.zMin + 1, pitch.top) };
  }

  /** Where a resident in this mood picks its spots on its side. */
  private ground(side: Side, mood: Mood): Region | null {
    const region = this.region(side);
    if (!region || side !== "right") return region;
    if (mood === "working") return this.workshopArea() ?? region;
    if (RESTING.has(mood)) return this.layout?.lounge?.area ?? region;
    const pitch = this.layout?.pitch;
    if (!pitch || !this.pitching) return region;
    return { ...region, zMax: Math.max(region.zMin + 1, pitch.top) };
  }

  private randomPoint(
    region: Region,
    near?: Vec2,
    radius = WANDER_RADIUS,
  ): Vec2 {
    // Clamped, or a point near someone outside the area lands outside it.
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
  private claimed(o: Agent): Vec2 {
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
      .map((o) => this.claimed(o));
    // The portal's doorway counts as taken, so nobody sets up in it.
    if (portal) others.push(portal, { x: portal.x, z: portal.z + 1.2 });
    const fire = side === "right" ? this.layout?.lounge?.fire : undefined;
    if (fire) others.push(fire);
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
      task: null,
      taskUntil: 0,
      haulLeg: 0,
      carrying: false,
      rest: Math.random() < 0.7 ? "seat" : "stroll",
      restUntil: this.now + rand(15, 45),
    };
    this.place(a, r);
    return a;
  }

  /** Straight onto its home ground: no walking in on first sight. */
  private place(a: Agent, r: Resident) {
    a.side = this.homeSide(r.mood);
    const region = this.ground(a.side, r.mood);
    if (!region) return;
    const slot =
      r.mood === "needs_you"
        ? this.attentionSpot(a)
        : r.mood === "shipping"
          ? (this.pitchPost(a)?.spot ?? null)
          : r.mood === "asleep" && a.side === "right"
            ? claim(this.naps, a.id, this.layout?.lounge?.naps ?? [])
            : null;
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

  /** Lets go of every shared place but the ones its mood still keeps. */
  private release(a: Agent, r: Resident) {
    const right = a.side === "right";
    if (r.mood !== "needs_you" || a.side !== "left")
      this.waitSlots.delete(a.id);
    if (r.mood !== "shipping" || !right || this.queue[0] === a.id) {
      this.chairs.delete(a.id);
    }
    if (!ROAMING.has(r.mood) || !right || a.rest !== "seat") {
      this.seats.delete(a.id);
    }
    if (r.mood !== "asleep" || !right) this.naps.delete(a.id);
    if (r.mood !== "working" || !right || a.task !== "board") {
      this.boards.delete(a.id);
    }
    if (r.mood !== "working") {
      a.task = null;
      a.carrying = false;
    }
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

    this.release(a, r);

    if (EGG.has(r.mood)) {
      a.target = null;
      this.setActivity(a, "egg");
      return;
    }
    if (a.side !== this.homeSide(r.mood)) {
      this.toPortal(a);
      return;
    }
    const region = this.ground(a.side, r.mood);
    if (!region) return;
    const holder = this.holder(a);

    switch (r.mood) {
      case "asleep": {
        const nap =
          a.side === "right" && !holder
            ? claim(this.naps, a.id, this.layout?.lounge?.naps ?? [])
            : null;
        if (nap) {
          this.head(a, nap, "sleep");
        } else {
          a.target = null;
          a.spot = null;
          this.setActivity(a, "sleep");
        }
        return;
      }
      case "needs_you": {
        const slot = this.attentionSpot(a);
        if (!slot && !a.spot) {
          a.spot = this.spacedPoint(a.id, a.side, region);
        }
        this.head(a, slot ?? a.spot!, "attention");
        return;
      }
      case "shipping": {
        const pitch = this.layout?.pitch;
        if (!pitch) break;
        const post = this.pitchPost(a);
        if (!post && !a.spot) {
          a.spot = this.spacedPoint(a.id, a.side, pitch.stage);
        }
        this.head(a, post?.spot ?? a.spot!, post?.post ?? "seated");
        return;
      }
      case "working":
        this.work(a, region, holder);
        return;
    }
    if (
      holder ||
      !this.layout?.lounge ||
      a.side !== "right" ||
      !RESTING.has(r.mood)
    ) {
      a.spot = null;
      this.roam(a, r, region);
    } else {
      this.lounge(a, r, region);
    }
  }

  /**
   * A working resident picks a task and keeps at it a while: its own work at a
   * spot of its own, hauling crates between the piles, or a whiteboard. On a
   * leash, it works at a spot within reach instead.
   */
  private work(a: Agent, area: Region, holder: Agent | undefined) {
    const shop = this.layout?.workshop;
    if (holder || !shop) {
      a.task = "station";
    } else if (!a.task || this.now > a.taskUntil) {
      const freeBoard = shop.boards.some(
        (_, i) => ![...this.boards.values()].includes(i),
      );
      const roll = Math.random();
      a.task =
        roll < 0.45 ? "station" : roll < 0.75 || !freeBoard ? "haul" : "board";
      a.taskUntil = this.now + rand(20, 40);
      a.spot = null;
      a.carrying = false;
      a.haulLeg = 0;
    }

    if (a.task === "board" && shop) {
      const stand = claim(this.boards, a.id, shop.boards);
      if (stand) {
        this.head(a, stand, "board");
        return;
      }
      a.task = "station";
    }
    if (a.task === "haul" && shop) {
      this.haul(a, shop);
      return;
    }
    // Re-picked too when the stage has gone up over it.
    if (!a.spot || this.stuck(a) || !inside(area, a.spot)) {
      a.spot = holder
        ? this.spacedPoint(a.id, a.side, area, holder.pos, LEASH_REACH)
        : this.spacedPoint(a.id, a.side, area, a.pos, 3);
    }
    this.head(a, a.spot, "work");
  }

  /** To the first pile for a crate, to the second to set it down, and again. */
  private haul(a: Agent, shop: Workshop) {
    if (a.activity === "pause" && this.now < a.until) return;
    const pile = shop.piles[a.haulLeg];
    // Each hauler has its own side of the pile, so they don't all queue.
    const at = {
      x: pile.x + Math.cos(a.phase) * 0.45,
      z: pile.z + Math.sin(a.phase) * 0.35,
    };
    if (dist(a.pos, at) < 0.3 || this.stuck(a)) {
      a.carrying = a.haulLeg === 0;
      a.haulLeg = a.haulLeg === 0 ? 1 : 0;
      this.pause(a, 0.5, 1);
      return;
    }
    this.walkTo(a, at, "haul");
  }

  /**
   * By the fire for a while, then up for a stroll round the lounge, and back.
   * No seat free, and it strolls until one is.
   */
  private lounge(a: Agent, r: Resident, area: Region) {
    const lounge = this.layout!.lounge!;
    if (this.now > a.restUntil) {
      a.rest = a.rest === "seat" ? "stroll" : "seat";
      a.restUntil = this.now + (a.rest === "seat" ? rand(20, 50) : rand(6, 14));
    }
    const seat =
      a.rest === "seat" ? claim(this.seats, a.id, lounge.seats) : null;
    if (seat) {
      a.spot = null;
      this.head(a, seat, "lounge");
      return;
    }
    if (a.rest === "seat") a.rest = "stroll";
    a.spot = null;
    this.roam(a, r, area);
  }

  /** Walk to a spot and stay there doing `post`. */
  private head(a: Agent, spot: Vec2, post: Post) {
    const reach = a.activity === post ? 0.5 : ARRIVE;
    // Close enough and blocked counts as there, rather than shoving forever.
    if (dist(a.pos, spot) < reach || (this.stuck(a) && dist(a.pos, spot) < 1)) {
      a.target = null;
      // Onto the furniture itself, not beside it.
      if (FURNISHED.has(post)) a.pos = { ...spot };
      this.setActivity(a, post);
    } else {
      this.walkTo(a, spot, post);
    }
  }

  /** This agent's slot on the left, claiming the best free one if it has none. */
  private attentionSpot(a: Agent): Vec2 | null {
    if (a.side !== "left") return null;
    return claim(this.waitSlots, a.id, this.layout?.attentionSlots ?? []);
  }

  /** The podium for whoever's turn it is, a chair of its own for the rest. */
  private pitchPost(a: Agent): { spot: Vec2; post: Post } | null {
    const pitch = this.layout?.pitch;
    if (a.side !== "right" || !pitch) return null;
    if (this.queue[0] === a.id) return { spot: pitch.podium, post: "pitch" };
    const chair = claim(this.chairs, a.id, pitch.chairs);
    return chair ? { spot: chair, post: "seated" } : null;
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
      const holder = this.holder(a);
      const target = holder
        ? this.spacedPoint(a.id, a.side, region, holder.pos, LEASH_REACH)
        : this.spacedPoint(a.id, a.side, region, a.pos);
      this.walkTo(a, target, "wander");
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
    const back = 0.6 + i * 0.45;
    const spread = (i % 2 ? 1 : -1) * 0.2;
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
    } else if (
      a.activity === "attention" ||
      a.activity === "pitch" ||
      a.activity === "seated"
    ) {
      wantFacing = 0;
    } else if (a.activity === "work") {
      wantFacing = 0.35 * Math.sin(a.phase);
    } else if (a.activity === "board") {
      // Back to the viewer, facing the board behind its spot.
      wantFacing = Math.PI;
    } else if (a.activity === "lounge") {
      const fire = this.layout?.lounge?.fire;
      if (fire) wantFacing = Math.atan2(fire.x - a.pos.x, fire.z - a.pos.z);
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

/** The slot `id` holds, claiming the best free one if it holds none. */
function claim(
  book: Map<string, number>,
  id: string,
  slots: Vec2[],
): Vec2 | null {
  let i = book.get(id);
  if (i === undefined) {
    const taken = new Set(book.values());
    i = slots.findIndex((_, j) => !taken.has(j));
    if (i < 0) return null;
    book.set(id, i);
  }
  return slots[i] ?? null;
}

function prune(book: Map<string, number>, length: number) {
  for (const [id, i] of book) if (i >= length) book.delete(id);
}

function clamp(n: number, min: number, max: number) {
  return Math.min(max, Math.max(min, n));
}

function inside(r: Region, p: Vec2) {
  return p.x >= r.xMin && p.x <= r.xMax && p.z >= r.zMin && p.z <= r.zMax;
}
