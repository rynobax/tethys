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
  /** The bottom of the left meadow, while anyone is Shipping. */
  pitch: Pitch | null;
  /** Furniture nobody walks through. */
  obstacles: Obstacle[];
  /** Larger rectangles nobody walks across, unless heading into them. */
  blocks: Region[];
  /** The top of the left meadow, where merged residents wait to be deleted. */
  lake: Lake | null;
}

/** Water off the window's edge, with a pier out into it and a boat at the end. */
export interface Lake {
  water: Region;
  /** The ground round it where the merged wait, the water a block within. */
  shore: Region;
  /** Where the pier meets the shore, and its end, where the boat is moored. */
  pierFoot: Vec2;
  pierEnd: Vec2;
  /** Where the boat is moored, alongside the pier's end. */
  berth: Vec2;
  /** The line for the boat, first by the boat: along the pier, then on up
   *  the shore. */
  dock: Vec2[];
  /** Off the edge of the window, where a boat sails to and is gone. */
  exit: Vec2;
}

/** A place to sit that faces a particular way. */
export interface Seat extends Vec2 {
  /** Radians about y; 0 faces the viewer. */
  facing: number;
}

/** A round footprint on the ground. */
export interface Obstacle extends Vec2 {
  r: number;
}

/** Where dismissed residents relax: on a lounger by the pool, or asleep on it. */
export interface Lounge {
  area: Region;
  pool: Region;
  /** Where a lounger's sitter is put, and which way it's turned. */
  seats: Seat[];
  /** A drink by each lounger, in the same order. */
  tables: Vec2[];
  /** Rings on the water, one sleeper each. */
  naps: Vec2[];
  parasols: Vec2[];
}

/** Where working residents work, at a task of their own or a shared one. */
export interface Workshop {
  area: Region;
  /** Where someone stands to write on each whiteboard. */
  boards: Vec2[];
  garden: Garden;
}

/** Plots where working residents grow crops, and the shed they're stored at. */
export interface Garden {
  /** One crop each. */
  plots: Vec2[];
  shed: Vec2;
  /** Where the harvest is stacked, in front of the shed. */
  store: Vec2;
}

/** A plot's crop, and how far grown: below 0 bare, 1 ripe. */
export interface Plot {
  crop: number;
  growth: number;
}

/** How many kinds of crop there are; the scene decides what each looks like. */
export const CROP_KINDS = 5;
/** Crates of harvest kept at the shed; past this the oldest goes. */
export const STORE_MAX = 9;

/** Where Shipping residents pitch, and the crowd they pitch to. */
export interface Pitch {
  /** Where the one pitching stands. */
  podium: Vec2;
  /** Where the rest wait their turn, best first: the row behind the podium,
   *  then the one behind that. */
  chairs: Vec2[];
  /** Each chair's row, 0 right behind the podium. */
  chairRows: number[];
  /** The stage with that many rows out, less one: it grows back as needed. */
  stages: Region[];
  crowd: Vec2[];
  /** The stage at its largest, with every row out. */
  stage: Region;
  /** Above the largest stage, where waiting residents keep. */
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
  | "board"
  /** At a garden plot, doing its `chore`. */
  | "garden"
  /** Selected: floating in its shaft of light, whatever it was doing. */
  | "chosen"
  /** Just merged: popping the champagne where it stands. */
  | "pop"
  /** Merged: in the line on the pier, waiting to be deleted. */
  | "queue"
  /** Deleted after merging: in a boat, sailing off across the lake. */
  | "sail";

type Post =
  | "attention"
  | "work"
  | "pitch"
  | "seated"
  | "lounge"
  | "sleep"
  | "board"
  | "garden"
  | "queue";
type Intent = "wander" | "portal" | "store" | "dock" | Post;

/** What a working resident is busy with, for a while. */
export type Task = "station" | "garden" | "board";

/** What a gardener does at its plot, which follows from how grown it is. */
export type Chore = "plant" | "water" | "harvest";

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
  /** Gardening: what it's doing at its plot, and the crop it's carrying to
   *  the shed. */
  chore: Chore | null;
  carrying: number | null;
  /** Lounging: on a lounger, or up for a stroll, and until when. */
  rest: "seat" | "stroll";
  restUntil: number;
  /** 0..1, how far it has risen off the ground into its light. */
  aloft: number;
  /** Popping the champagne until then, having just been merged. */
  popUntil: number;
  /** Deleted and on its way out: to the pier's foot, then along it. */
  voyage: "dock" | "pier" | null;
}

const PORTAL_SECONDS = 0.6;
const ARRIVE = 0.2;
/** No closer to the target in this long and it gives up. */
const STUCK_SECONDS = 1.2;
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
const RESTING: ReadonlySet<Mood> = new Set(["idle", "asleep"]);
/** How long a resident pops its champagne for once it's merged. */
const POP_SECONDS = 3.5;
const SPEED_SAIL = 0.9;
/** How long planting and picking take, and watering at most. */
const PLANT_SECONDS = 2.2;
const HARVEST_SECONDS = 1.8;
const WATER_SECONDS = 3;
/** Growth a second: a crop ripens in a few minutes alone, far sooner watered. */
const GROW_RATE = 1 / 240;
const WATER_RATE = 0.12;
/** A crate of harvest is taken from the shed this often. */
const COLLECT_SECONDS = 50;

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
  private plotsHeld = new Map<string, number>();
  private docks = new Map<string, number>();
  private plots: Plot[] = [];
  /** The harvest stacked at the shed, oldest first, and when the oldest goes. */
  private stored: number[] = [];
  private collectAt = 0;
  private dt = 0;
  /** Shipping residents in turn order; the first is the one pitching. */
  private queue: string[] = [];
  private speakerSince = 0;
  /** Whether anyone is on stage, so the rest keep off it. */
  private pitching = false;
  private now = 0;
  /** Deleted residents still sailing away, as they last were. */
  private leaving = new Map<string, Resident>();
  /** Ones whose boat has left the window, for the scene to let go of. */
  private departed: string[] = [];

  setLayout(layout: Layout) {
    this.layout = layout;
    prune(this.waitSlots, layout.attentionSlots.length);
    prune(this.chairs, layout.pitch?.chairs.length ?? 0);
    prune(this.seats, layout.lounge?.seats.length ?? 0);
    prune(this.naps, layout.lounge?.naps.length ?? 0);
    prune(this.boards, layout.workshop?.boards.length ?? 0);
    prune(this.docks, layout.lake?.dock.length ?? 0);
    const plots = layout.workshop?.garden.plots.length ?? 0;
    prune(this.plotsHeld, plots);
    // The first time, a garden already under way rather than bare earth.
    const fresh = this.plots.length === 0;
    this.plots = Array.from(
      { length: plots },
      (_, i) =>
        this.plots[i] ?? {
          crop: Math.floor(Math.random() * CROP_KINDS),
          growth: fresh && Math.random() < 0.6 ? rand(0, 1.2) : -1,
        },
    );
    for (const p of this.plots) p.growth = Math.min(1, p.growth);
    for (const a of this.agents.values()) this.settle(a);
  }

  sync(residents: Resident[]) {
    const before = this.residents;
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
    for (const [id, a] of this.agents) {
      if (this.residents.has(id) || this.leaving.has(id)) continue;
      for (const book of this.books()) book.delete(id);
      // A merged one waiting by the lake leaves by boat; anyone else just goes.
      const was = before.get(id);
      if (was?.mood === "shipped" && this.canSail(a)) {
        this.leaving.set(id, { ...was, selected: false });
        a.voyage = "dock";
      } else {
        this.agents.delete(id);
      }
    }
    for (const r of residents) {
      const a = this.agents.get(r.id);
      // Deleting it was undone: off the boat, or the pier, and back.
      if (a && this.leaving.delete(r.id)) {
        a.voyage = null;
        a.target = null;
        this.setActivity(a, "pause");
      }
      const was = before.get(r.id)?.mood;
      // Merged from the stage or from waiting to be merged: pop the cork.
      if (
        a &&
        r.mood === "shipped" &&
        (was === "shipping" || was === "needs_you")
      ) {
        a.popUntil = this.now + POP_SECONDS;
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
    this.dt = dt;
    for (const p of this.plots) {
      if (p.growth >= 0) p.growth = Math.min(1, p.growth + dt * GROW_RATE);
    }
    if (this.stored.length > 0 && this.now > this.collectAt) {
      this.stored.shift();
      this.collectAt = this.now + COLLECT_SECONDS;
    }
    // The speaker takes a seat at the back of the queue and the next gets up,
    // unless it's the chosen one, which holds the floor.
    if (
      this.queue.length > 1 &&
      this.now - this.speakerSince > PITCH_TURN_SECONDS &&
      this.agents.get(this.queue[0])?.activity !== "chosen"
    ) {
      // The speaker sits down in the chair its successor gets up from.
      const [speaker, next] = this.queue;
      const seat = this.chairs.get(next);
      this.chairs.delete(next);
      if (seat !== undefined) this.chairs.set(speaker, seat);
      this.queue.push(this.queue.shift()!);
      this.speakerSince = this.now;
    }
    for (const a of this.agents.values()) {
      if (this.leaving.has(a.id)) {
        this.voyage(a);
        this.move(a, dt, a.activity === "sail" ? SPEED_SAIL : SPEED_STROLL);
        continue;
      }
      const r = this.residents.get(a.id);
      if (!r) continue;
      this.think(a, r);
      const hurry = r.mood === "needs_you" || a.intent === "portal";
      this.move(a, dt, hurry ? SPEED_HURRY : SPEED_STROLL + (1 - a.size));
    }
    this.separate();
    for (const a of this.agents.values()) this.clear(a);
  }

  time() {
    return this.now;
  }

  /** Whether someone has this stage chair, so it's drawn. */
  chairTaken(index: number): boolean {
    for (const i of this.chairs.values()) if (i === index) return true;
    return false;
  }

  /** How many rows of chairs are out: enough for everyone waiting to pitch. */
  chairRowsOut(): number {
    const pitch = this.layout?.pitch;
    if (!pitch) return 0;
    const waiting = Math.max(0, this.queue.length - 1);
    const needed = pitch.chairRows[Math.min(waiting, pitch.chairs.length) - 1];
    return Math.max(1, (needed ?? 0) + 1);
  }

  /** The stage as it stands, with only the rows that are out. */
  stageNow(): Region | null {
    const pitch = this.layout?.pitch;
    return pitch ? pitch.stages[this.chairRowsOut() - 1] : null;
  }

  /** Whether someone has this pool ring, so it isn't also drawn empty. */
  napTaken(index: number): boolean {
    for (const i of this.naps.values()) if (i === index) return true;
    return false;
  }

  /** The residents deleted but still sailing away, as they last were. */
  sailors(): Resident[] {
    return [...this.leaving.values()];
  }

  /** Whether any boat is out, so the one at the pier isn't drawn too. */
  sailing(): boolean {
    for (const id of this.leaving.keys()) {
      if (this.agents.get(id)?.activity === "sail") return true;
    }
    return false;
  }

  /** Those whose boat has left the window since the last call. */
  takeDeparted(): string[] {
    const out = this.departed;
    this.departed = [];
    return out;
  }

  /** Asleep on its ring in the pool, rather than wherever it dropped off. */
  afloat(id: string): boolean {
    const a = this.agents.get(id);
    const i = this.naps.get(id);
    const ring = i === undefined ? undefined : this.layout?.lounge?.naps[i];
    return !!a && !!ring && a.activity === "sleep" && dist(a.pos, ring) < 0.05;
  }

  private books() {
    return [
      this.waitSlots,
      this.chairs,
      this.seats,
      this.naps,
      this.boards,
      this.plotsHeld,
      this.docks,
    ];
  }

  private region(side: Side): Region | null {
    return this.layout?.regions[side] ?? null;
  }

  /** Waiting and pitching on the left, the rest on the right, if there's room. */
  private homeSide(mood: Mood): Side {
    const lakeside = mood === "shipped" && !!this.layout?.lake;
    const want: Side =
      mood === "needs_you" || mood === "shipping" || lakeside
        ? "left"
        : "right";
    return this.region(want) || !this.region(other(want)) ? want : other(want);
  }

  /** Where a resident in this mood picks its spots on its side. */
  private ground(side: Side, mood: Mood): Region | null {
    const region = this.region(side);
    if (!region) return null;
    if (side === "right") {
      if (mood === "working") return this.layout?.workshop?.area ?? region;
      if (RESTING.has(mood)) return this.layout?.lounge?.area ?? region;
      return region;
    }
    const lake = this.layout?.lake;
    if (mood === "shipped" && lake) return lake.shore;
    const pitch = this.layout?.pitch;
    if (!pitch || !this.pitching || mood === "shipping") return region;
    const top = this.stageNow()!.zMin - 0.4;
    return { ...region, zMax: Math.max(region.zMin + 1, top) };
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
    const furniture = this.layout?.obstacles ?? [];
    const blocks = (this.layout?.blocks ?? []).map((b) => grow(b, 0.3));
    // Drawn again when it lands on a block, which can be half the ground.
    const candidate = () => {
      let p = this.randomPoint(region, near, radius);
      for (let i = 0; i < 12 && blocks.some((b) => inside(b, p)); i++) {
        p = this.randomPoint(region, near, radius);
      }
      return p;
    };
    let best = candidate();
    let bestScore = -Infinity;
    for (let i = 0; i < CANDIDATES; i++) {
      const p = i === 0 ? best : candidate();
      const score = Math.min(
        ...others.map((o) => dist(o, p)),
        ...furniture.map((o) => dist(o, p) - o.r - 0.3),
        ...blocks.map((b) => (inside(b, p) ? -10 : Infinity)),
      );
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
      chore: null,
      carrying: null,
      rest: Math.random() < 0.7 ? "seat" : "stroll",
      restUntil: this.now + rand(15, 45),
      aloft: 0,
      popUntil: 0,
      voyage: null,
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
            : this.loungerFor(a, r);
    const parent = this.parentOf(a);
    a.pos = slot
      ? { x: slot.x, z: slot.z }
      : this.spacedPoint(
          a.id,
          a.side,
          region,
          parent?.placed && parent.side === a.side ? parent.pos : undefined,
          1.5,
        );
    a.placed = true;
  }

  /** A free lounger for a resident that would rather be sitting, if any. */
  private loungerFor(a: Agent, r: Resident): Vec2 | null {
    const lounge = this.layout?.lounge;
    if (!lounge || a.side !== "right" || !RESTING.has(r.mood)) return null;
    if (r.mood === "asleep" || a.rest !== "seat") return null;
    return claim(this.seats, a.id, lounge.seats);
  }

  /** After a resize: back onto its ground, or over to the other meadow. */
  private settle(a: Agent) {
    const r = this.residents.get(a.id);
    if (!r) return;
    if (!a.placed || !this.region(a.side)) {
      a.target = null;
      a.spot = null;
      this.place(a, r);
    }
    // Its own ground, not just the meadow, so nobody is left on the stage.
    const region = this.ground(a.side, r.mood);
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
    if (r.mood !== "shipping" || right || this.queue[0] === a.id) {
      this.chairs.delete(a.id);
    }
    if (!ROAMING.has(r.mood) || !right || a.rest !== "seat") {
      this.seats.delete(a.id);
    }
    if (r.mood !== "asleep" || !right) this.naps.delete(a.id);
    if (r.mood !== "shipped" || right) this.docks.delete(a.id);
    if (r.mood !== "working" || !right || a.task !== "board") {
      this.boards.delete(a.id);
    }
    if (r.mood !== "working" || !right || a.task !== "garden") {
      this.plotsHeld.delete(a.id);
      a.chore = null;
    }
    if (r.mood !== "working") {
      a.task = null;
      a.carrying = null;
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

    if (this.now < a.popUntil) {
      a.target = null;
      a.intent = null;
      this.setActivity(a, "pop");
      return;
    }
    if (EGG.has(r.mood)) {
      a.target = null;
      this.setActivity(a, "egg");
      return;
    }
    if (this.basking(a, r)) return;
    if (a.side !== this.homeSide(r.mood)) {
      this.toPortal(a);
      return;
    }
    const region = this.ground(a.side, r.mood);
    if (!region) return;
    switch (r.mood) {
      case "asleep": {
        const nap =
          a.side === "right"
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
          a.spot = this.spacedPoint(a.id, a.side, this.stageNow()!);
        }
        this.head(a, post?.spot ?? a.spot!, post?.post ?? "seated");
        return;
      }
      case "working":
        this.work(a, region);
        return;
      case "shipped": {
        // In line for the boat, or about the shore once the line is full.
        const place = this.dockSpot(a);
        if (!place) break;
        a.spot = null;
        this.head(a, place, "queue");
        return;
      }
    }
    if (!this.layout?.lounge || a.side !== "right" || !RESTING.has(r.mood)) {
      a.spot = null;
      this.roam(a, region);
    } else {
      this.lounge(a, region);
    }
  }

  /** Whether a resident deleted now can take the boat rather than vanish. */
  private canSail(a: Agent): boolean {
    return (
      !!this.layout?.lake &&
      a.placed &&
      a.side === "left" &&
      a.activity !== "egg" &&
      a.activity !== "portal_out" &&
      a.activity !== "portal_in"
    );
  }

  /** To the pier's foot, out along it to the boat, and away across the lake. */
  private voyage(a: Agent) {
    const lake = this.layout?.lake;
    if (!lake) return this.depart(a);
    if (a.activity === "sail") {
      if (dist(a.pos, lake.exit) < 0.3) this.depart(a);
      return;
    }
    if (a.voyage === "dock") {
      // Blocked on the way, or already in the line on the pier, it makes for
      // the pier's end from where it is.
      const onPier =
        Math.abs(a.pos.z - lake.pierEnd.z) < 0.3 &&
        a.pos.x < lake.pierFoot.x + 0.1;
      if (dist(a.pos, lake.pierFoot) < 0.3 || onPier || this.stuck(a)) {
        a.voyage = "pier";
      } else {
        this.walkTo(a, lake.pierFoot, "dock");
        return;
      }
    }
    if (dist(a.pos, lake.pierEnd) < 0.15) {
      // Stepped down into the boat.
      a.pos = { ...lake.berth };
      a.target = { ...lake.exit };
      this.setActivity(a, "sail");
    } else {
      this.walkTo(a, lake.pierEnd, "dock");
    }
  }

  private depart(a: Agent) {
    this.agents.delete(a.id);
    this.leaving.delete(a.id);
    this.departed.push(a.id);
  }

  /**
   * The chosen one drops whatever it's doing and basks in its light, right
   * where it is, for as long as it's chosen. Nothing moves it on meanwhile,
   * not even a change of mood: that waits until Ryan looks elsewhere.
   */
  private basking(a: Agent, r: Resident): boolean {
    if (!r.selected) return false;
    a.target = null;
    a.intent = null;
    this.setActivity(a, "chosen");
    return true;
  }

  /**
   * A working resident picks a task and keeps at it a while: its own work at a
   * spot of its own, the garden, or a whiteboard.
   */
  private work(a: Agent, area: Region) {
    const shop = this.layout?.workshop;
    if (!shop) {
      a.task = "station";
    } else if (!a.task || (this.now > a.taskUntil && a.carrying === null)) {
      const freeBoard = shop.boards.some(
        (_, i) => ![...this.boards.values()].includes(i),
      );
      const roll = Math.random();
      a.task =
        roll < 0.35 ? "station" : roll < 0.5 && freeBoard ? "board" : "garden";
      a.taskUntil =
        this.now + (a.task === "garden" ? rand(40, 70) : rand(20, 40));
      a.spot = null;
    }

    if (a.task === "board" && shop) {
      const stand = claim(this.boards, a.id, shop.boards);
      if (stand) {
        this.head(a, stand, "board");
        return;
      }
      a.task = "station";
    }
    if (a.task === "garden" && shop) {
      if (this.garden(a, shop.garden)) return;
      // Every plot taken: its own work for a while instead.
      a.task = "station";
    }
    // Re-picked too when a resize has moved the workshop.
    if (!a.spot || this.stuck(a) || !inside(area, a.spot)) {
      a.spot = this.spacedPoint(a.id, a.side, area, a.pos, 3);
    }
    this.head(a, a.spot, "work");
  }

  /**
   * Picks a ripe plot, plants a bare one or waters a growing one, and carries
   * each crop it picks to the shed. A plot is one gardener's at a time. False
   * when there's no plot free.
   */
  private garden(a: Agent, garden: Garden): boolean {
    if (a.activity === "pause" && this.now < a.until) return true;
    if (a.carrying !== null) {
      // Each its own side of the stack, so they don't all queue.
      const at = {
        x: garden.store.x + Math.cos(a.phase) * 0.35,
        z: garden.store.z + 0.45 + Math.abs(Math.sin(a.phase)) * 0.2,
      };
      if (dist(a.pos, at) < 0.3 || (this.stuck(a) && dist(a.pos, at) < 1)) {
        if (this.stored.length === 0) {
          this.collectAt = this.now + COLLECT_SECONDS;
        }
        this.stored.push(a.carrying);
        if (this.stored.length > STORE_MAX) this.stored.shift();
        a.carrying = null;
        this.pause(a, 0.6, 1.2);
      } else {
        this.walkTo(a, at, "store");
      }
      return true;
    }
    const i = this.plotFor(a, garden);
    if (i === null) return false;
    const plot = this.plots[i];
    a.chore = plot.growth < 0 ? "plant" : plot.growth < 1 ? "water" : "harvest";
    const at = garden.plots[i];
    // Behind its plot, facing out over it.
    this.head(a, { x: at.x, z: at.z - 0.42 }, "garden");
    if (a.activity !== "garden") return true;
    const busy = this.now - a.since;
    let done = false;
    if (a.chore === "plant" && busy > PLANT_SECONDS) {
      plot.crop = Math.floor(Math.random() * CROP_KINDS);
      plot.growth = 0;
      done = true;
    } else if (a.chore === "water") {
      plot.growth = Math.min(1, plot.growth + this.dt * WATER_RATE);
      done = busy > WATER_SECONDS || plot.growth >= 1;
    } else if (a.chore === "harvest" && busy > HARVEST_SECONDS) {
      a.carrying = plot.crop;
      plot.growth = -1;
      done = true;
    }
    if (done) {
      this.plotsHeld.delete(a.id);
      a.chore = null;
      this.pause(a, 0.3, 0.8);
    }
    return true;
  }

  /** Its plot, or the best free one: ripe first, then bare, then growing. */
  private plotFor(a: Agent, garden: Garden): number | null {
    const held = this.plotsHeld.get(a.id);
    if (held !== undefined) return held;
    const taken = new Set(this.plotsHeld.values());
    let best: number | null = null;
    let bestScore = Infinity;
    this.plots.forEach((p, i) => {
      if (taken.has(i)) return;
      const rank = p.growth >= 1 ? 0 : p.growth < 0 ? 1 : 2;
      // A rank is worth a few steps, so nobody crosses the garden for it.
      const score = rank * 3 + dist(a.pos, garden.plots[i]);
      if (score < bestScore) {
        best = i;
        bestScore = score;
      }
    });
    if (best !== null) this.plotsHeld.set(a.id, best);
    return best;
  }

  /** Its place in the line for the boat, moving up whenever one ahead is
   *  free; `null` when the line is full. */
  private dockSpot(a: Agent): Vec2 | null {
    const dock = this.layout?.lake?.dock ?? [];
    const held = this.docks.get(a.id);
    const taken = new Set(this.docks.values());
    const free = dock.findIndex((_, i) => !taken.has(i));
    if (free >= 0 && (held === undefined || free < held)) {
      this.docks.set(a.id, free);
    }
    const i = this.docks.get(a.id);
    return i === undefined ? null : dock[i];
  }

  /** Its place in the line for the boat, so alternate name tags can stagger. */
  dockIndex(id: string): number | undefined {
    return this.docks.get(id);
  }

  /** A plot's crop and growth, for the scene. */
  plot(index: number): Plot | undefined {
    return this.plots[index];
  }

  /** The harvest stacked at the shed, oldest first. */
  storedCrops(): readonly number[] {
    return this.stored;
  }

  /**
   * On a lounger for a while, then up for a stroll round the pool, and back.
   * No seat free, and it strolls until one is.
   */
  private lounge(a: Agent, area: Region) {
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
    this.roam(a, area);
  }

  /** Walk to a spot and stay there doing `post`. */
  private head(a: Agent, spot: Vec2, post: Post) {
    const reach = a.activity === post ? 0.5 : ARRIVE;
    // Close enough and blocked counts as there, rather than shoving forever.
    if (dist(a.pos, spot) < reach || (this.stuck(a) && dist(a.pos, spot) < 1)) {
      a.target = null;
      // Onto the furniture itself, not beside it.
      if (FURNISHED.has(post)) a.pos = { x: spot.x, z: spot.z };
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
    if (a.side !== "left" || !pitch) return null;
    if (this.queue[0] === a.id) return { spot: pitch.podium, post: "pitch" };
    // A row put away as the queue shrank: forward into one that's out.
    const held = this.chairs.get(a.id);
    if (held !== undefined && pitch.chairRows[held] >= this.chairRowsOut()) {
      this.chairs.delete(a.id);
    }
    const chair = claim(this.chairs, a.id, pitch.chairs);
    return chair ? { spot: chair, post: "seated" } : null;
  }

  private roam(a: Agent, region: Region) {
    if (a.activity === "walk") {
      if (a.target && dist(a.pos, a.target) < ARRIVE) {
        this.pause(a, 1.5, 5);
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

    this.walkTo(a, this.spacedPoint(a.id, a.side, region, a.pos), "wander");
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

  private move(a: Agent, dt: number, speed: number) {
    const rise = a.activity === "chosen" ? 1 : 0;
    a.aloft += (rise - a.aloft) * Math.min(1, dt * (rise ? 2.5 : 4));
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
    if ((a.activity === "walk" || a.activity === "sail") && a.target) {
      const dx = a.target.x - a.pos.x;
      const dz = a.target.z - a.pos.z;
      const d = Math.hypot(dx, dz);
      if (d < a.bestDist - 0.05) {
        a.bestDist = d;
        a.bestAt = this.now;
      }
      if (d > 1e-4) {
        const stepLen = Math.min(d, speed * dt);
        const dir = this.steer(a, dx / d, dz / d);
        a.pos.x += dir.x * stepLen;
        a.pos.z += dir.z * stepLen;
        wantFacing = Math.atan2(dir.x, dir.z);
      }
    } else if (
      a.activity === "attention" ||
      a.activity === "pitch" ||
      a.activity === "seated" ||
      a.activity === "chosen" ||
      a.activity === "pop"
    ) {
      wantFacing = 0;
    } else if (a.activity === "lounge") {
      const i = this.seats.get(a.id);
      wantFacing =
        i === undefined ? 0 : (this.layout?.lounge?.seats[i]?.facing ?? 0);
    } else if (a.activity === "work") {
      wantFacing = 0.35 * Math.sin(a.phase);
    } else if (a.activity === "garden") {
      wantFacing = 0;
    } else if (a.activity === "queue") {
      // Turned towards the boat, half to the viewer.
      wantFacing = -1.1;
    } else if (a.activity === "board") {
      // Back to the viewer, facing the board behind its spot.
      wantFacing = Math.PI;
    }
    if (wantFacing !== null) {
      let diff = wantFacing - a.facing;
      while (diff > Math.PI) diff -= Math.PI * 2;
      while (diff < -Math.PI) diff += Math.PI * 2;
      a.facing += diff * Math.min(1, dt * 8);
    }
  }

  /** The furniture in this agent's way, leaving out whatever it's walking to. */
  private obstacles(a: Agent): Obstacle[] {
    const all = this.layout?.obstacles ?? [];
    const goal = a.target;
    if (!goal) return all;
    return all.filter((o) => dist(goal, o) > this.clearance(a, o) + 0.1);
  }

  /** The blocks in this agent's way, grown by its own girth. */
  private blocks(a: Agent): Region[] {
    const goal = a.target;
    const margin = a.size * 0.3;
    // A goal just outside a block but within the margin is still heading in.
    return (this.layout?.blocks ?? [])
      .map((b) => grow(b, margin))
      .filter((b) => !goal || !inside(b, goal));
  }

  private clearance(a: Agent, o: Obstacle) {
    return o.r + a.size * 0.3;
  }

  /**
   * Veers off a straight line to the target round anything just ahead on it,
   * to whichever side it's already further round.
   */
  private steer(a: Agent, x: number, z: number): Vec2 {
    // Round a block by the shortest way past its corners.
    const goal = a.target;
    if (goal) {
      for (const b of this.blocks(a)) {
        if (!crosses(a.pos, goal, b)) continue;
        const next = detour(a.pos, goal, b);
        if (next) {
          const d = dist(a.pos, next) || 1;
          x = (next.x - a.pos.x) / d;
          z = (next.z - a.pos.z) / d;
        }
      }
    }
    let sx = x;
    let sz = z;
    for (const o of this.obstacles(a)) {
      const ox = o.x - a.pos.x;
      const oz = o.z - a.pos.z;
      const ahead = ox * x + oz * z;
      if (ahead <= 0 || ahead > 1.4) continue;
      // How far the obstacle sits off the line, to the walker's right.
      const off = ox * z - oz * x;
      const clear = this.clearance(a, o) + 0.1;
      if (Math.abs(off) >= clear) continue;
      const away = off > 0 || (off === 0 && Math.sin(a.phase) > 0) ? -1 : 1;
      const push = (1 - Math.abs(off) / clear) * 2;
      sx += z * away * push;
      sz += -x * away * push;
    }
    const n = Math.hypot(sx, sz) || 1;
    return { x: sx / n, z: sz / n };
  }

  /** Out of any furniture it has been pushed or pulled into. */
  private clear(a: Agent) {
    if (a.activity !== "walk" && a.activity !== "pause") return;
    for (const b of this.blocks(a)) {
      if (!inside(grow(b, -0.001), a.pos)) continue;
      // Out by the nearest edge.
      const exits = [
        { d: a.pos.x - b.xMin, p: { x: b.xMin, z: a.pos.z } },
        { d: b.xMax - a.pos.x, p: { x: b.xMax, z: a.pos.z } },
        { d: a.pos.z - b.zMin, p: { x: a.pos.x, z: b.zMin } },
        { d: b.zMax - a.pos.z, p: { x: a.pos.x, z: b.zMax } },
      ];
      a.pos = exits.reduce((m, e) => (e.d < m.d ? e : m)).p;
    }
    for (const o of this.obstacles(a)) {
      const dx = a.pos.x - o.x;
      const dz = a.pos.z - o.z;
      const d = Math.hypot(dx, dz);
      const min = this.clearance(a, o);
      if (d >= min) continue;
      const ux = d > 1e-5 ? dx / d : 1;
      const uz = d > 1e-5 ? dz / d : 0;
      a.pos.x = o.x + ux * min;
      a.pos.z = o.z + uz * min;
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

function grow(r: Region, by: number): Region {
  return {
    xMin: r.xMin - by,
    xMax: r.xMax + by,
    zMin: r.zMin - by,
    zMax: r.zMax + by,
  };
}

function corners(r: Region): Vec2[] {
  return [
    { x: r.xMin, z: r.zMin },
    { x: r.xMax, z: r.zMin },
    { x: r.xMin, z: r.zMax },
    { x: r.xMax, z: r.zMax },
  ];
}

/**
 * The first corner to head for on the shortest way from `from` to `to` round
 * `block`, by one corner or two; a corner it's already at doesn't count.
 */
function detour(from: Vec2, to: Vec2, block: Region): Vec2 | null {
  const inner = grow(block, -0.01);
  const clear = (p: Vec2, q: Vec2) => !crosses(p, q, inner);
  const cs = corners(grow(block, 0.05)).filter((c) => dist(c, from) > 0.08);
  let best: Vec2 | null = null;
  let bestLength = Infinity;
  for (const c1 of cs) {
    if (!clear(from, c1)) continue;
    const first = dist(from, c1);
    if (clear(c1, to) && first + dist(c1, to) < bestLength) {
      bestLength = first + dist(c1, to);
      best = c1;
    }
    for (const c2 of cs) {
      if (c2 === c1 || !clear(c1, c2) || !clear(c2, to)) continue;
      const length = first + dist(c1, c2) + dist(c2, to);
      if (length < bestLength) {
        bestLength = length;
        best = c1;
      }
    }
  }
  return best;
}

/** Whether the segment from `p` to `q` passes through the inside of `r`. */
function crosses(p: Vec2, q: Vec2, r: Region): boolean {
  let t0 = 0;
  let t1 = 1;
  const dx = q.x - p.x;
  const dz = q.z - p.z;
  const sides: [number, number][] = [
    [-dx, p.x - r.xMin],
    [dx, r.xMax - p.x],
    [-dz, p.z - r.zMin],
    [dz, r.zMax - p.z],
  ];
  for (const [step, room] of sides) {
    if (step === 0) {
      if (room <= 0) return false;
      continue;
    }
    const t = room / step;
    if (step < 0) {
      if (t > t1) return false;
      t0 = Math.max(t0, t);
    } else {
      if (t < t0) return false;
      t1 = Math.min(t1, t);
    }
  }
  return t1 - t0 > 1e-6;
}

function inside(r: Region, p: Vec2) {
  return p.x >= r.xMin && p.x <= r.xMax && p.z >= r.zMin && p.z <= r.zMax;
}
