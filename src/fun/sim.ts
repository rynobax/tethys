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

import { bake, findPath, sees, walkable, type Grid, type Path } from "./paths";
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

export interface Waiting {
  /** Open ground on a fine lattice, every place one might stand. */
  spots: Vec2[];
  /** What they gather round. */
  centre: Vec2;
  /** The room one takes on screen, a name tag wide and a figure tall. */
  foot: { x: number; z: number };
}

export interface Layout {
  /** `null` when the window leaves no room on that side. */
  regions: Record<Side, Region | null>;
  portals: Record<Side, Vec2 | null>;
  /** Where waiting residents may stand; `null` with no left meadow. */
  waiting: Waiting | null;
  /** The right meadow's zones; `null` when it's too short to divide. */
  lounge: Lounge | null;
  workshop: Workshop | null;
  /** Along the right meadow's front, where parents quilt; `null` with the
   *  workshop. */
  porch: Porch | null;
  /** The bottom of the left meadow, while anyone is Shipping. */
  pitch: Pitch | null;
  /** Furniture nobody walks through. */
  obstacles: Obstacle[];
  /** Larger rectangles nobody walks across, unless heading into them. */
  blocks: Region[];
  /** The top of the left meadow, where the ready to merge line up for
   *  tickets and the merged wait to be deleted. */
  lake: Lake | null;
  /** Down the left meadow's outer edge, one per folder in order, as fit. */
  houses: House[];
  /** The ground the houses take, which nobody waits on. */
  street: Region | null;
}

/** A folder's house: just its front, facing the viewer. */
export interface House {
  /** The middle of the front's base. */
  front: Vec2;
  /** On the step, where its folk go in and come out. */
  door: Vec2;
  /** Out in front, where its folk who need you wait rather than go in. */
  doorstep: Vec2[];
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
  /** On the shore below the water, its window turned to the line for it. */
  office: Vec2;
  /** The line of the ready to merge for tickets, first at the window. */
  tickets: Vec2[];
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

/** Where idle residents relax: on a lounger by the pool, or asleep on it. */
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

/** Where parents sit and quilt, a square for each task done. */
export interface Porch {
  area: Region;
  /** Best first; each a rocking chair, and its quilting frame in front. */
  places: QuiltingPlace[];
}

export interface QuiltingPlace {
  /** Where the quilter sits, facing the viewer. */
  seat: Vec2;
  /** The middle of the frame, between the quilter and the viewer. */
  frame: Vec2;
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
  /** In a line: on the pier once merged, waiting to be deleted, or at the
   *  ticket office once ready to merge. */
  | "queue"
  /** Deleted after merging: in a boat, sailing off across the lake. */
  | "sail"
  /** A parent in its rocking chair, stitching its quilt. */
  | "quilt"
  /** A parent up on its chair, cheering a task just done. */
  | "cheer"
  /** Its folder shut: going in at its house's door, inside, and back out. */
  | "home_in"
  | "home"
  | "home_out";

type Post =
  | "attention"
  | "work"
  | "pitch"
  | "seated"
  | "lounge"
  | "sleep"
  | "board"
  | "garden"
  | "queue"
  | "quilt";
type Intent = "wander" | "portal" | "store" | "dock" | "home" | Post;

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
  /** The way to `target`, and the target it was found for. */
  path: Path | null;
  pathFor: Vec2 | null;
  until: number;
  /** Where it works, or waits when there's no slot to wait in. */
  spot: Vec2 | null;
  /** The least way it has had left to `target`, and when, to notice being
   *  stuck; and whether it has already found a new way for being stuck. */
  bestLeft: number;
  bestAt: number;
  repathed: boolean;
  /** A working resident's current task, and when it moves on to another. */
  task: Task | null;
  taskUntil: number;
  /** Gardening: what it's doing at its plot, and the crop it's carrying to
   *  the shed. */
  chore: Chore | null;
  carrying: number | null;
  /** Lounging: on a lounger, or up for a stroll, and until when. */
  /** Idle by the pool: on a lounger, asleep on a ring, or strolling. */
  rest: Rest;
  restUntil: number;
  /** 0..1, how far it has risen off the ground into its light. */
  aloft: number;
  /** Popping the champagne until then, having just been merged. */
  popUntil: number;
  /** Cheering on its chair until then, a task just done. */
  cheerUntil: number;
  /** The house it's inside, if it is. */
  indoors: number | null;
  /** Deleted and on its way out: to the pier's foot, then along it. */
  voyage: "dock" | "pier" | null;
}

const PORTAL_SECONDS = 0.6;
const ARRIVE = 0.2;
/** No nearer the target along its way in this long and it's stuck. */
const STUCK_SECONDS = 3;
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
const FURNISHED: ReadonlySet<Post> = new Set([
  "sleep",
  "lounge",
  "seated",
  "quilt",
]);
/** Moods a parent spends quilting; the rest send it where they always do. */
const QUILTING: ReadonlySet<Mood> = new Set(["idle", "working"]);
/** Going in at a door, inside, or coming out: nobody to bump into. */
const INDOORS: ReadonlySet<Activity> = new Set(["home_in", "home", "home_out"]);
/** How soon after a house goes up its folk are simply inside it. */
const HOUSE_SETTLE = 1;
/** How long a parent cheers each task done. */
const CHEER_SECONDS = 2.6;
/** The ones Ryan has dismissed, who go to the lounge. */
const RESTING: ReadonlySet<Mood> = new Set(["idle"]);

type Rest = "seat" | "float" | "stroll";
const REST_SECONDS: Record<Rest, () => number> = {
  seat: () => rand(20, 50),
  float: () => rand(25, 60),
  stroll: () => rand(6, 14),
};
/** Mostly a lounger, sometimes a nap on the water, now and then a stroll. */
function pickRest(stroll = true): Rest {
  const roll = Math.random();
  if (roll < 0.6) return "seat";
  return roll < 0.85 || !stroll ? "float" : "stroll";
}
/** How long a resident pops its champagne for once it's merged. */
const POP_SECONDS = 3.5;
/** How loosely the waiting gather, in tag widths given or taken from the
 *  middle; past about one it's no longer a cluster. */
const WAITING_LOOSENESS = 0.8;
/** A waiting place survives a new layout if open ground is still this near. */
const WAITING_KEPT = 0.2;
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
  /** Where each waiting resident chose to stand, kept until it's done. */
  private waitSpots = new Map<string, Vec2>();
  private chairs = new Map<string, number>();
  private seats = new Map<string, number>();
  private naps = new Map<string, number>();
  private boards = new Map<string, number>();
  private plotsHeld = new Map<string, number>();
  private docks = new Map<string, number>();
  private tickets = new Map<string, number>();
  /** Out in front of a house: which one, and the place among those there. */
  private doorsteps = new Map<string, { house: number; place: number }>();
  /** Each meadow's walkable ground, for finding the way across it. */
  private grids: Record<Side, Grid | null> = { left: null, right: null };
  /** When each house went up: one just built takes its folk straight in. */
  private housesBuilt: number[] = [];
  /** Each quilter's place on the porch. */
  private quilters = new Map<string, number>();
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
    this.housesBuilt = layout.houses.map(
      (_, i) => this.housesBuilt[i] ?? this.now,
    );
    this.layout = layout;
    this.grids = { left: bake(layout, "left"), right: bake(layout, "right") };
    for (const [id, d] of this.doorsteps) {
      if (d.house >= layout.houses.length) this.doorsteps.delete(id);
    }
    // Kept wherever it's still open ground to wait on.
    const ground = layout.waiting?.spots ?? [];
    for (const [id, p] of this.waitSpots) {
      if (!ground.some((q) => dist(p, q) < WAITING_KEPT)) {
        this.waitSpots.delete(id);
      }
    }
    prune(this.chairs, layout.pitch?.chairs.length ?? 0);
    prune(this.seats, layout.lounge?.seats.length ?? 0);
    prune(this.naps, layout.lounge?.naps.length ?? 0);
    prune(this.boards, layout.workshop?.boards.length ?? 0);
    prune(this.docks, layout.lake?.dock.length ?? 0);
    prune(this.tickets, layout.lake?.tickets.length ?? 0);
    prune(this.quilters, layout.porch?.places.length ?? 0);
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
    for (const a of this.agents.values()) {
      a.path = null;
      this.settle(a);
    }
  }

  sync(residents: Resident[]) {
    const before = this.residents;
    this.residents = new Map(residents.map((r) => [r.id, r]));
    const pitches = (r: Resident) => r.mood === "shipping";
    this.pitching = residents.some(pitches);
    const shipping = new Set(residents.filter(pitches).map((r) => r.id));
    const speaker = this.queue[0];
    this.queue = this.queue.filter((id) => shipping.has(id));
    for (const id of [...shipping].sort()) {
      if (!this.queue.includes(id)) this.queue.push(id);
    }
    if (this.queue[0] !== speaker) this.speakerSince = this.now;
    for (const [id, a] of this.agents) {
      if (this.residents.has(id) || this.leaving.has(id)) continue;
      for (const book of this.books()) book.delete(id);
      this.doorsteps.delete(id);
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
        (was === "shipping" || was === "needs_you" || was === "ready")
      ) {
        a.popUntil = this.now + POP_SECONDS;
      }
      // Another task done: a cheer from its chair.
      const done = before.get(r.id)?.progress?.done;
      if (a && done !== undefined && (r.progress?.done ?? 0) > done) {
        a.cheerUntil = this.now + CHEER_SECONDS;
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

  private books(): Map<string, unknown>[] {
    return [
      this.waitSpots,
      this.chairs,
      this.seats,
      this.naps,
      this.boards,
      this.plotsHeld,
      this.docks,
      this.tickets,
      this.quilters,
    ];
  }

  private region(side: Side): Region | null {
    return this.layout?.regions[side] ?? null;
  }

  /** Waiting and pitching on the left, the rest on the right, if there's room. */
  private homeSide(mood: Mood): Side {
    const lakeside = mood === "shipped" && !!this.layout?.lake;
    const want: Side =
      mood === "needs_you" ||
      mood === "shipping" ||
      mood === "ready" ||
      lakeside
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
    // The merged and the ready to merge, in their lines or about the shore.
    if ((mood === "shipped" || mood === "ready") && lake) return lake.shore;
    // Off the street, so nobody stands about in front of the houses.
    const street = this.layout?.street;
    const open =
      street && street.xMax < region.xMax - 1
        ? { ...region, xMin: Math.max(region.xMin, street.xMax) }
        : region;
    const pitch = this.layout?.pitch;
    if (!pitch || !this.pitching || mood === "shipping") return open;
    const top = this.stageNow()!.zMin - 0.4;
    return { ...open, zMax: Math.max(open.zMin + 1, top) };
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
    const grid = this.grids[side];
    const open = (p: Vec2) => !grid || walkable(grid, p);
    // Drawn again when it lands off walkable ground, which can be half of it.
    const candidate = () => {
      let p = this.randomPoint(region, near, radius);
      for (let i = 0; i < 12 && !open(p); i++) {
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
        open(p) ? Infinity : -10,
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
      path: null,
      pathFor: null,
      until: this.now + rand(0.2, 2),
      spot: null,
      bestLeft: Infinity,
      bestAt: this.now,
      repathed: false,
      task: null,
      taskUntil: 0,
      chore: null,
      carrying: null,
      rest: pickRest(),
      restUntil: this.now + rand(15, 45),
      aloft: 0,
      popUntil: 0,
      cheerUntil: 0,
      indoors: null,
      voyage: null,
    };
    this.place(a, r);
    return a;
  }

  /** Straight onto its home ground: no walking in on first sight. */
  private place(a: Agent, r: Resident) {
    if (r.home !== null && this.houseFor(r)) {
      if (r.mood === "needs_you") this.outside(a, r);
      else this.inside(a, r.home);
      a.placed = true;
      return;
    }
    a.side = this.homeSide(r.mood);
    const region = this.ground(a.side, r.mood);
    if (!region) return;
    const slot =
      r.mood === "needs_you"
        ? this.attentionSpot(a, region)
        : r.mood === "shipping"
          ? (this.pitchPost(a)?.spot ?? null)
          : (this.quiltingPlace(a, r)?.seat ?? this.loungerFor(a, r));
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
    if (a.rest === "float") return claim(this.naps, a.id, lounge.naps);
    if (a.rest !== "seat") return null;
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
    // On the porch, which is nobody's ground but the quilters'.
    if (this.quilters.has(a.id)) return;
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
      a.bestLeft = Infinity;
      a.bestAt = this.now;
      a.repathed = false;
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

  /** Lets go of every shared place but the ones its mood still keeps. */
  private release(a: Agent, r: Resident) {
    const right = a.side === "right";
    if (r.mood !== "needs_you") this.waitSpots.delete(a.id);
    if (r.mood !== "shipping" || right || this.queue[0] === a.id) {
      this.chairs.delete(a.id);
    }
    if (!ROAMING.has(r.mood) || !right || a.rest !== "seat") {
      this.seats.delete(a.id);
    }
    if (!ROAMING.has(r.mood) || !right || a.rest !== "float") {
      this.naps.delete(a.id);
    }
    if (r.mood !== "shipped" || right) this.docks.delete(a.id);
    if (r.mood !== "ready" || right) this.tickets.delete(a.id);
    // Kept while it basks, which takes it out of its house, so nobody else
    // is sent to stand where it is.
    if (r.mood !== "needs_you" || (!this.houseFor(r) && !r.selected)) {
      this.doorsteps.delete(a.id);
    }
    if (!r.quilts || !QUILTING.has(r.mood) || !right) {
      this.quilters.delete(a.id);
    }
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
    if (this.homeward(a, r)) return;

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
    if (this.quilt(a, r)) return;
    const region = this.ground(a.side, r.mood);
    if (!region) return;
    switch (r.mood) {
      case "needs_you": {
        this.head(a, this.attentionSpot(a, region), "attention");
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
      case "shipped":
      case "ready": {
        // In line for the boat, or for tickets, or about the shore once the
        // line is full.
        const place =
          r.mood === "shipped"
            ? this.placeInLine(a, this.docks, this.layout?.lake?.dock)
            : this.placeInLine(a, this.tickets, this.layout?.lake?.tickets);
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
      // Already in the line on the pier, it makes for the pier's end from
      // where it is.
      const onPier =
        Math.abs(a.pos.z - lake.pierEnd.z) < 0.3 &&
        a.pos.x < lake.pierFoot.x + 0.1;
      if (dist(a.pos, lake.pierFoot) < 0.3 || onPier) {
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
    if (!a.spot || !inside(area, a.spot)) {
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
      if (dist(a.pos, at) < 0.3) {
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

  /** Its place in a line, moving up whenever one ahead is free; `null` when
   *  the line is full. */
  private placeInLine(
    a: Agent,
    book: Map<string, number>,
    line: Vec2[] = [],
  ): Vec2 | null {
    const held = book.get(a.id);
    const taken = new Set(book.values());
    const free = line.findIndex((_, i) => !taken.has(i));
    if (free >= 0 && (held === undefined || free < held)) {
      book.set(a.id, free);
    }
    const i = book.get(a.id);
    return i === undefined ? null : line[i];
  }

  /** Its folder's house, when the folder is shut and the house is out. */
  private houseFor(r: Resident): House | undefined {
    return r.home === null ? undefined : this.layout?.houses[r.home];
  }

  /**
   * Its folder shut: in at its house's door and out of sight, letting go of
   * everything, or out in front waving while it needs you. Opened, out the
   * door again and off to wherever it belongs. False when none of that is
   * going on.
   */
  private homeward(a: Agent, r: Resident): boolean {
    const house = this.houseFor(r);
    const waits = r.mood === "needs_you";
    if (a.activity === "home_out") {
      if (this.now - a.since >= PORTAL_SECONDS) this.pause(a, 0.2, 0.6);
      return true;
    }
    if (a.indoors !== null && (!house || r.home !== a.indoors || waits)) {
      const door =
        this.layout?.houses[a.indoors]?.door ?? this.layout?.houses[0]?.door;
      a.indoors = null;
      a.target = null;
      a.spot = null;
      if (door) {
        a.side = "left";
        a.pos = { x: door.x, z: door.z };
        a.facing = 0;
        this.setActivity(a, "home_out");
        return true;
      }
      a.placed = false;
      this.place(a, r);
      return false;
    }
    if (!house) return false;
    for (const book of this.books()) book.delete(a.id);
    // Its house only just up, as the folders load after a restart: it was
    // home all along, rather than off across the meadow.
    const settling =
      this.now - (this.housesBuilt[r.home!] ?? -Infinity) < HOUSE_SETTLE;
    if (waits && settling && a.activity !== "attention") {
      this.outside(a, r);
      return true;
    }
    if (!waits && settling && a.activity !== "home") {
      this.inside(a, r.home!);
      return true;
    }
    a.task = null;
    a.chore = null;
    a.carrying = null;
    a.spot = null;
    if (waits) {
      if (a.side !== "left") this.toPortal(a);
      else this.head(a, this.doorstepSpot(a, r), "attention");
      return true;
    }
    if (a.activity === "home") return true;
    if (a.activity === "home_in") {
      if (this.now - a.since >= PORTAL_SECONDS) {
        a.indoors = r.home;
        this.setActivity(a, "home");
      }
      return true;
    }
    if (a.side !== "left") {
      this.toPortal(a);
      return true;
    }
    // An egg can't walk: it's simply indoors.
    const egg = EGG.has(r.mood);
    const there = dist(a.pos, house.door);
    if (egg || there < ARRIVE) {
      a.target = null;
      a.pos = { x: house.door.x, z: house.door.z };
      a.facing = Math.PI;
      if (egg) {
        a.indoors = r.home;
        this.setActivity(a, "home");
      } else {
        this.setActivity(a, "home_in");
      }
      return true;
    }
    this.walkTo(a, house.door, "home");
    return true;
  }

  /** Its place out in front of its house, the first free among those there
   *  who need you; past the last, a little further out each time round. */
  private doorstepSpot(a: Agent, r: Resident): Vec2 {
    const house = r.home!;
    const steps = this.layout!.houses[house].doorstep;
    let held = this.doorsteps.get(a.id);
    if (held?.house !== house) {
      const taken = new Set(
        [...this.doorsteps.values()]
          .filter((d) => d.house === house)
          .map((d) => d.place),
      );
      let place = 0;
      while (taken.has(place)) place++;
      held = { house, place };
      this.doorsteps.set(a.id, held);
    }
    const step = steps[held.place % steps.length];
    const round = Math.floor(held.place / steps.length);
    return { x: step.x + round * 0.5, z: step.z + round * 0.3 };
  }

  /** Out in front of its house at once, with no walk there. */
  private outside(a: Agent, r: Resident) {
    a.side = "left";
    a.pos = this.doorstepSpot(a, r);
    a.target = null;
    a.intent = null;
    a.spot = null;
    a.facing = 0;
    this.setActivity(a, "attention");
  }

  /** Indoors at once, with no walk home. */
  private inside(a: Agent, house: number) {
    const door = this.layout!.houses[house].door;
    a.side = "left";
    a.pos = { x: door.x, z: door.z };
    a.target = null;
    a.intent = null;
    a.spot = null;
    a.indoors = house;
    this.setActivity(a, "home");
  }

  /** A parent's place on the porch, claiming a free one if it has none. */
  private quiltingPlace(a: Agent, r: Resident): QuiltingPlace | null {
    const porch = this.layout?.porch;
    if (!porch || !r.quilts || !QUILTING.has(r.mood) || a.side !== "right") {
      return null;
    }
    if (!this.quilters.has(a.id)) {
      const taken = new Set(this.quilters.values());
      const free = porch.places.findIndex((_, i) => !taken.has(i));
      if (free < 0) return null;
      this.quilters.set(a.id, free);
    }
    return porch.places[this.quilters.get(a.id)!];
  }

  /**
   * A parent idle or working sits on the porch and quilts, up on its chair to
   * cheer each task done. False with no place free, to do as anyone would.
   */
  private quilt(a: Agent, r: Resident): boolean {
    const place = this.quiltingPlace(a, r);
    if (!place) return false;
    // Whatever else it had going is put down.
    this.seats.delete(a.id);
    this.boards.delete(a.id);
    this.plotsHeld.delete(a.id);
    a.task = null;
    a.chore = null;
    a.carrying = null;
    a.spot = null;
    if (a.activity === "cheer" && this.now < a.cheerUntil) return true;
    this.head(a, place.seat, "quilt");
    if (a.activity === "quilt" && this.now < a.cheerUntil) {
      this.setActivity(a, "cheer");
    }
    return true;
  }

  /** Who is inside this house. */
  indoors(house: number): Resident[] {
    const out: Resident[] = [];
    for (const a of this.agents.values()) {
      const r = this.residents.get(a.id);
      if (r && a.indoors === house) out.push(r);
    }
    return out;
  }

  /** Whoever quilts at this place on the porch, if anyone. */
  quilter(index: number): Resident | undefined {
    for (const [id, i] of this.quilters) {
      if (i !== index) continue;
      const a = this.agents.get(id);
      return a?.activity === "quilt" || a?.activity === "cheer"
        ? this.residents.get(id)
        : undefined;
    }
    return undefined;
  }

  /** Its place in the line for the boat or for tickets, so alternate name
   *  tags can stagger. */
  lineIndex(id: string): number | undefined {
    return this.docks.get(id) ?? this.tickets.get(id);
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
      // A stroll between each sit or nap.
      a.rest = a.rest === "stroll" ? pickRest(false) : "stroll";
      a.restUntil = this.now + REST_SECONDS[a.rest]();
    }
    const seat =
      a.rest === "seat" ? claim(this.seats, a.id, lounge.seats) : null;
    if (seat) {
      a.spot = null;
      this.head(a, seat, "lounge");
      return;
    }
    const ring =
      a.rest === "float" ? claim(this.naps, a.id, lounge.naps) : null;
    if (ring) {
      a.spot = null;
      this.head(a, ring, "sleep");
      return;
    }
    // Nothing free: a stroll until there is.
    a.rest = "stroll";
    a.spot = null;
    this.roam(a, area);
  }

  /** Walk to a spot and stay there doing `post`. */
  private head(a: Agent, spot: Vec2, post: Post) {
    // Nudged a little off it once there is still there.
    const reach = a.activity === post ? 0.5 : ARRIVE;
    if (dist(a.pos, spot) < reach) {
      a.target = null;
      // Onto the furniture itself, not beside it.
      if (FURNISHED.has(post)) a.pos = { x: spot.x, z: spot.z };
      this.setActivity(a, post);
    } else {
      this.walkTo(a, spot, post);
    }
  }

  /**
   * Where it waits on the left, choosing if it hasn't: free ground roughly
   * near the middle, with room for its tag among the others there. Once
   * chosen it stays, nobody shuffling up to fill a gap. Past the room, where
   * it covers others' tags least; with no left meadow, a spaced point.
   */
  private attentionSpot(a: Agent, region: Region): Vec2 {
    const held = this.waitSpots.get(a.id);
    if (held) return held;
    const waiting = a.side === "left" ? this.layout?.waiting : null;
    const grid = this.grids.left;
    const others = [...this.waitSpots.values()];
    const foot = waiting?.foot;
    // How much its tag would cover others', in tags: none is clear ground.
    const crowding = (p: Vec2) => {
      let sum = 0;
      for (const o of others) {
        sum +=
          Math.max(0, 1 - Math.abs(o.x - p.x) / foot!.x) *
          Math.max(0, 1 - Math.abs(o.z - p.z) / foot!.z);
      }
      return sum;
    };
    let best: Vec2 | null = null;
    let bestScore = Infinity;
    for (const p of waiting?.spots ?? []) {
      if (grid && !walkable(grid, p)) continue;
      // In tag widths from the middle, give or take, so they gather loosely;
      // crowding outweighs any of that, so it's only where there's no room.
      const score =
        Math.hypot(
          (p.x - waiting!.centre.x) / foot!.x,
          (p.z - waiting!.centre.z) / foot!.z,
        ) +
        rand(0, WAITING_LOOSENESS) +
        crowding(p) * 100;
      if (score < bestScore) {
        best = p;
        bestScore = score;
      }
    }
    best ??= this.spacedPoint(a.id, a.side, region);
    this.waitSpots.set(a.id, best);
    return best;
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
      if (a.target && dist(a.pos, a.target) < ARRIVE) this.pause(a, 1.5, 5);
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
      if (dist(a.pos, portal) < ARRIVE) this.setActivity(a, "portal_out");
      return;
    }
    if (a.activity === "portal_out") return;
    this.walkTo(a, { ...portal }, "portal");
  }

  private move(a: Agent, dt: number, speed: number) {
    const rise = a.activity === "chosen" ? 1 : 0;
    a.aloft += (rise - a.aloft) * Math.min(1, dt * (rise ? 2.5 : 4));
    if (a.activity === "portal_out" || a.activity === "home_in") {
      a.portalScale = Math.max(0, 1 - (this.now - a.since) / PORTAL_SECONDS);
      return;
    }
    if (a.activity === "portal_in" || a.activity === "home_out") {
      a.portalScale = Math.min(1, (this.now - a.since) / PORTAL_SECONDS);
      return;
    }
    if (a.activity === "home") {
      a.portalScale = 0;
      return;
    }
    a.portalScale = 1;

    let wantFacing: number | null = null;
    if ((a.activity === "walk" || a.activity === "sail") && a.target) {
      // A boat sails straight across the water; anyone else finds the way.
      const aim = a.activity === "walk" ? this.waypoint(a) : a.target;
      const dx = aim.x - a.pos.x;
      const dz = aim.z - a.pos.z;
      const d = Math.hypot(dx, dz);
      if (d > 1e-4) {
        const stepLen = Math.min(d, speed * dt);
        a.pos.x += (dx / d) * stepLen;
        a.pos.z += (dz / d) * stepLen;
        wantFacing = Math.atan2(dx, dz);
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
    } else if (
      a.activity === "garden" ||
      a.activity === "quilt" ||
      a.activity === "cheer"
    ) {
      wantFacing = 0;
    } else if (a.activity === "queue") {
      // Turned towards the boat, or the ticket window, half to the viewer.
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

  /**
   * The next place to make for on the way to its target, finding the way when
   * the target has moved, it has been shoved off it, or it's stuck. Stuck
   * again after finding a new way, it's put on its target, with a warning:
   * that's a bug in the layout or here, not something to hide.
   */
  private waypoint(a: Agent): Vec2 {
    const target = a.target!;
    const grid = this.grids[a.side];
    if (!grid) return target;
    if (!a.path || !a.pathFor || dist(a.pathFor, target) > 1e-6) {
      this.findWay(a, grid);
    }
    let points = a.path!.points;
    const through = a.path!.through;
    // Shoved off its way, so the next corner is out of sight.
    if (
      points.length > 1 &&
      walkable(grid, a.pos, through) &&
      !sees(grid, a.pos, points[0], through)
    ) {
      points = this.findWay(a, grid);
    }
    // On to the next corner as soon as it can see past this one.
    while (
      points.length > 1 &&
      (dist(a.pos, points[0]) < 0.12 || sees(grid, a.pos, points[1], through))
    ) {
      points.shift();
    }
    let left = dist(a.pos, points[0]);
    for (let k = 1; k < points.length; k++) {
      left += dist(points[k - 1], points[k]);
    }
    if (left < a.bestLeft - 0.05) {
      a.bestLeft = left;
      a.bestAt = this.now;
    } else if (this.now - a.bestAt > STUCK_SECONDS) {
      if (a.repathed) {
        console.warn(
          `fun: ${a.id} stuck at (${a.pos.x.toFixed(2)}, ${a.pos.z.toFixed(2)}) ` +
            `on its way to (${target.x.toFixed(2)}, ${target.z.toFixed(2)}); put there`,
        );
        a.pos = { x: target.x, z: target.z };
        return target;
      }
      a.repathed = true;
      a.bestAt = this.now;
      points = this.findWay(a, grid);
    }
    return points[0];
  }

  private findWay(a: Agent, grid: Grid): Vec2[] {
    const target = a.target!;
    a.path = findPath(grid, a.pos, target) ?? {
      points: [{ x: target.x, z: target.z }],
      through: -1,
    };
    a.pathFor = { x: target.x, z: target.z };
    return a.path.points;
  }

  /**
   * Nobody stands inside anybody else. Whoever is settled holds its ground,
   * and is no wall to anyone walking, who passes by; anyone standing about
   * makes way. Nobody is pushed off walkable ground, or further than half a
   * step at a time: a crowd can't hold a walker back, nor shove anyone.
   */
  private separate() {
    const list = [...this.agents.values()].filter(
      (a) => !INDOORS.has(a.activity),
    );
    const walking = (a: Agent) => a.activity === "walk";
    const movable = (a: Agent) => walking(a) || a.activity === "pause";
    for (let i = 0; i < list.length; i++) {
      for (let j = i + 1; j < list.length; j++) {
        const a = list[i];
        const b = list[j];
        if (a.side !== b.side) continue;
        const ma = movable(a);
        const mb = movable(b);
        if (!ma && !mb) continue;
        if ((walking(a) && !mb) || (walking(b) && !ma)) continue;
        const min = (a.size + b.size) * 0.4;
        const dx = b.pos.x - a.pos.x;
        const dz = b.pos.z - a.pos.z;
        const d = Math.hypot(dx, dz);
        if (d >= min || d < 1e-5) continue;
        const push = (min - d) / (ma && mb ? 2 : 1);
        // Sideways as well as apart, so two walking head-on slide past.
        const ux = dx / d - (dz / d) * 0.5;
        const uz = dz / d + (dx / d) * 0.5;
        if (ma) this.shove(a, -ux * push, -uz * push);
        if (mb) this.shove(b, ux * push, uz * push);
      }
    }
  }

  private shove(a: Agent, dx: number, dz: number) {
    const most = SPEED_STROLL * 0.5 * this.dt;
    const length = Math.hypot(dx, dz);
    if (length > most) {
      dx *= most / length;
      dz *= most / length;
    }
    const to = { x: a.pos.x + dx, z: a.pos.z + dz };
    const grid = this.grids[a.side];
    if (grid && walkable(grid, a.pos) && !walkable(grid, to)) return;
    a.pos = to;
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
