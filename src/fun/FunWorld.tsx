import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type RefObject,
} from "react";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { ContextMenu, type WorkspaceMenuActions } from "../Sidebar";
import { useTheme } from "../theme";
import type { Folder, Workspace, WorkspaceId } from "../types";
import { ANIMALS, type AnimalName } from "./animals";
import { YOUNG_HEAD } from "./animals/rig";
import {
  Character,
  SEAT_LIFT,
  CHOSEN_HOVER,
  CHEER_LIFT,
  QUILT_LIFT,
  type CharacterHandlers,
  type Voice,
} from "./Character";
import { allocate, loadAnimals, sameAnimals, saveAnimals } from "./family";
import { Houses } from "./Houses";
import { toResidents, type Resident } from "./residents";
import { Scenery } from "./Scenery";
import {
  EnvironmentContext,
  OUTDOORS,
  useToday,
  type Environment,
} from "./environment";
import { PALETTES, type Palette } from "./palette";
import { Sim, type Activity } from "./sim";
import { CLEAR, useWeather, weathered, type Weather } from "./forecast";
import { WeatherLayer } from "./Weather";
import {
  CAMERA_DISTANCE,
  PITCH,
  PIXELS_PER_UNIT,
  STAGE_HEIGHT,
  computeStage,
  type Stage,
} from "./stage";
import "./fun.css";

/** Smooth everywhere, without doubling the work on a 120Hz display. */
const MAX_FPS = 60;

export interface FunWorldProps {
  workspaces: Workspace[];
  needsTurn: (w: Workspace) => boolean;
  working: (w: Workspace) => boolean;
  selectedId: WorkspaceId | null;
  /** Null when the ground is clicked, to clear the selection. */
  onSelect: (id: WorkspaceId | null) => void;
  /** In the sidebar's order: a house each, shut while the folder is. */
  folders: Folder[];
  onSetFolderCollapsed: (id: Folder["id"], collapsed: boolean) => void;
  /** The sidebar's context menu actions, for the same menu on a resident. */
  menuActions: WorkspaceMenuActions;
  /** The terminal panel; the meadows are whatever it leaves either side. */
  panelRef: RefObject<HTMLElement | null>;
  /**
   * For screenshots: given, the world stands still and is handed a clock to
   * move it by hand (`scripts/fun-shot.mjs`). Never set in the app.
   */
  onManualClock?: (clock: ManualClock) => void;
  /**
   * For the preview and screenshots: this weather rather than the weather
   * where Ryan is. Never set in the app.
   */
  weather?: Weather;
  /** For the preview and screenshots: this environment rather than today's. */
  environment?: Environment;
}

/** Time in the world, moved by hand. */
export interface ManualClock {
  /** Runs the world on by `seconds`, then draws one frame of where it is. */
  step: (seconds: number) => Promise<void>;
  /** For a script to read where everyone is. */
  sim: Sim;
}

/**
 * How long a resident's state must hold before the world shows it. Tethys
 * passes through states for a moment (a hook landing before the check that
 * clears it), and each would otherwise send it walking off and back.
 */
const SETTLE_MS = 2000;

/** What changing state means for a resident, shown together. */
type Settled = Pick<Resident, "mood" | "readyToMerge" | "note">;

const settledKey = (s: Settled) => `${s.mood}|${s.readyToMerge}|${s.note}`;

/**
 * Each resident as last shown until its new state has held for `ms`. One
 * first seen shows its state at once.
 */
function useSettled(residents: Resident[], ms: number): Resident[] {
  const shown = useRef(new Map<string, Settled>());
  /** A state not yet shown, and when it was first seen. */
  const pending = useRef(new Map<string, { key: string; at: number }>());
  const [now, setNow] = useState(0);
  const settled = useMemo(() => {
    const t = performance.now();
    const next = new Map<string, Settled>();
    const out = residents.map((r) => {
      const state: Settled = {
        mood: r.mood,
        readyToMerge: r.readyToMerge,
        note: r.note,
      };
      const key = settledKey(state);
      const held = shown.current.get(r.id);
      const waiting = pending.current.get(r.id);
      if (!held || ms <= 0 || settledKey(held) === key) {
        pending.current.delete(r.id);
        next.set(r.id, state);
        return r;
      }
      if (waiting?.key !== key) pending.current.set(r.id, { key, at: t });
      if (t - pending.current.get(r.id)!.at >= ms) {
        pending.current.delete(r.id);
        next.set(r.id, state);
        return r;
      }
      next.set(r.id, held);
      return { ...r, ...held };
    });
    for (const id of pending.current.keys()) {
      if (!next.has(id)) pending.current.delete(id);
    }
    shown.current = next;
    return out;
    // `now` only wakes it to let a state that has held through.
  }, [residents, ms, now]);
  useEffect(() => {
    const ats = [...pending.current.values()].map((p) => p.at + ms);
    if (ats.length === 0) return;
    const due = Math.min(...ats) - performance.now();
    const timer = setTimeout(() => setNow(performance.now()), due + 20);
    return () => clearTimeout(timer);
  }, [settled, ms]);
  return settled;
}

/** Steps a manual clock takes the sim in, matching the display's pace. */
const MANUAL_STEP = 1 / 60;

/** Each workspace's animal, kept across restarts. */
function useAnimals(
  workspaces: Workspace[],
): ReadonlyMap<WorkspaceId, AnimalName> {
  const held = useRef<Map<WorkspaceId, AnimalName> | null>(null);
  const animals = useMemo(() => {
    held.current ??= loadAnimals();
    const next = allocate(
      held.current,
      workspaces
        .filter((w) => !w.deleted_at)
        .map((w) => ({ id: w.id, parent: w.parent })),
    );
    if (!sameAnimals(next, held.current)) held.current = next;
    return held.current;
  }, [workspaces]);
  useEffect(() => saveAnimals(animals), [animals]);
  return animals;
}

/** The world starts below the title strip, which fullscreen drops. */
function titlebarHeight(): number {
  const h = getComputedStyle(document.documentElement).getPropertyValue(
    "--titlebar-h",
  );
  return parseFloat(h) || 0;
}

function useStage(
  panelRef: RefObject<HTMLElement | null>,
  folders: number,
): Stage | null {
  const [stage, setStage] = useState<Stage | null>(null);
  useLayoutEffect(() => {
    const measure = () => {
      // Widened over the right meadow for a PR or artifact (`App.css`): the
      // world keeps the layout it had, carrying on under the pane.
      if (panelRef.current?.querySelector(".main-tab-body")) return;
      const panel = panelRef.current?.getBoundingClientRect();
      if (!panel) return;
      setStage(
        computeStage(
          window.innerWidth,
          window.innerHeight - titlebarHeight(),
          { left: panel.left, right: panel.right },
          folders,
        ),
      );
    };
    const observer = new ResizeObserver(measure);
    // The panel can mount after the world does, so wait for it to be there.
    let raf = 0;
    const watch = () => {
      if (!panelRef.current) {
        raf = requestAnimationFrame(watch);
        return;
      }
      measure();
      observer.observe(panelRef.current);
    };
    watch();
    window.addEventListener("resize", measure);
    return () => {
      cancelAnimationFrame(raf);
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [panelRef, folders]);
  return stage;
}

/** The environment's light or dark look, by the app's theme. */
function usePalette(environment: Environment): Palette {
  const theme = useTheme();
  return useMemo(() => {
    const { light, dark } = PALETTES[environment];
    const bg = theme?.colors.background;
    if (bg) {
      const { r, g, b } = new THREE.Color(bg);
      return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.35 ? dark : light;
    }
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? dark
      : light;
  }, [theme, environment]);
}

/**
 * Drives every frame off the display's own refresh, capped at `MAX_FPS`, and
 * at full rate whether the window is focused or not: the world is for
 * glancing at while working somewhere else. The browser stops calling it
 * while the window is hidden.
 */
function FrameDriver({
  sim,
  onDeparted,
  onManualClock,
}: {
  sim: Sim;
  onDeparted: () => void;
  onManualClock?: (clock: ManualClock) => void;
}) {
  const advance = useThree((s) => s.advance);
  // By hand: the sim is stepped here in fixed steps without drawing each one,
  // then a frame is drawn at the clock's new time. React gets a turn first and
  // between steps, so a change of state or a departure lands as it would live.
  useEffect(() => {
    if (!onManualClock) return;
    let now = 0;
    const settle = () => new Promise((r) => setTimeout(r, 0));
    onManualClock({
      sim,
      step: async (seconds) => {
        await settle();
        for (let left = seconds; left > 1e-6; left -= MANUAL_STEP) {
          sim.step(Math.min(MANUAL_STEP, left));
          if (sim.takeDeparted().length) {
            onDeparted();
            await settle();
          }
        }
        now += seconds;
        await settle();
        advance(now);
      },
    });
  }, [onManualClock, sim, onDeparted, advance]);
  useEffect(() => {
    if (onManualClock) return;
    let raf = 0;
    let last = 0;
    const interval = 1000 / MAX_FPS;
    const tick = (now: number) => {
      raf = requestAnimationFrame(tick);
      // A few ms of slack, so 60Hz renders every refresh and 120Hz every other.
      if (now - last < interval - 4) return;
      last = now;
      // R3F reads a manual timestamp in seconds, not rAF's milliseconds.
      advance(now / 1000);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [advance, onManualClock]);
  return null;
}

function SimDriver({
  sim,
  onDeparted,
  manual,
}: {
  sim: Sim;
  /** Some deleted resident's boat has sailed off the window. */
  onDeparted: () => void;
  /** Stepped by a manual clock instead, so frames only draw. */
  manual: boolean;
}) {
  // Mounted before every Character, so they all pose the same step.
  useFrame((_, delta) => {
    if (manual) return;
    sim.step(Math.min(delta, 0.1));
    if (sim.takeDeparted().length) onDeparted();
  });
  return null;
}

function Aim() {
  const camera = useThree((s) => s.camera);
  useLayoutEffect(() => {
    camera.position.set(
      0,
      Math.sin(PITCH) * CAMERA_DISTANCE,
      Math.cos(PITCH) * CAMERA_DISTANCE,
    );
    camera.lookAt(0, 0, 0);
    // Labels project before the first render would otherwise refresh it.
    camera.updateMatrixWorld();
    camera.updateProjectionMatrix();
  }, [camera]);
  return null;
}

/** Pins each DOM label above its resident's head. */
const COMPACT: ReadonlySet<Activity> = new Set([
  "seated",
  "lounge",
  "sleep",
  "queue",
  "quilt",
  "cheer",
]);
/** How far every other tag in a line is lifted, clear of its neighbours'. */
const DOCK_STAGGER_PX = 26;

/**
 * A shaft of light from above the screen onto the chosen one, brighter while
 * Ryan types, with motes drifting down it.
 */
function DivineBeam({
  sim,
  selectedId,
  voice,
}: {
  sim: Sim;
  selectedId: WorkspaceId | null;
  voice: Voice;
}) {
  const group = useRef<THREE.Group>(null);
  const outer = useRef<THREE.MeshBasicMaterial>(null);
  const core = useRef<THREE.MeshBasicMaterial>(null);
  const motes = useRef<(THREE.Mesh | null)[]>([]);
  const glow = useRef(0);
  useFrame((state, delta) => {
    const g = group.current;
    const a = selectedId ? sim.agents.get(selectedId) : undefined;
    if (!g) return;
    g.visible = !!a && a.placed;
    if (!a) return;
    g.position.set(a.pos.x, 0, a.pos.z);
    g.scale.setScalar(Math.max(0.001, a.portalScale));
    const hearing = performance.now() - voice.lastKeyAt < 900;
    glow.current += ((hearing ? 1 : 0) - glow.current) * Math.min(1, delta * 4);
    const t = state.clock.elapsedTime;
    if (outer.current) {
      outer.current.opacity =
        0.07 + 0.08 * glow.current + Math.sin(t * 2) * 0.01;
    }
    if (core.current) core.current.opacity = 0.08 + 0.14 * glow.current;
    motes.current.forEach((m, i) => {
      if (!m) return;
      const fall = (t * (0.9 + glow.current * 1.5) + i * 0.61) % 4;
      m.position.set(
        Math.sin(i * 2.4) * 0.28,
        4.2 - fall,
        Math.cos(i * 2.4) * 0.28,
      );
      m.rotation.y = t * 2 + i;
    });
  });
  return (
    <group ref={group} visible={false}>
      <mesh position-y={15}>
        <cylinderGeometry args={[0.75, 0.5, 30, 20, 1, true]} />
        <meshBasicMaterial
          ref={outer}
          color="#fff1c1"
          transparent
          blending={THREE.AdditiveBlending}
          depthWrite={false}
          side={THREE.DoubleSide}
        />
      </mesh>
      <mesh position-y={15}>
        <cylinderGeometry args={[0.3, 0.22, 30, 14, 1, true]} />
        <meshBasicMaterial
          ref={core}
          color="#ffffff"
          transparent
          blending={THREE.AdditiveBlending}
          depthWrite={false}
          side={THREE.DoubleSide}
        />
      </mesh>
      {Array.from({ length: 10 }, (_, i) => (
        <mesh
          key={i}
          ref={(m) => {
            motes.current[i] = m;
          }}
        >
          <octahedronGeometry args={[0.035]} />
          <meshBasicMaterial color="#fff6d6" />
        </mesh>
      ))}
    </group>
  );
}

/** Clear space kept between a name tag and the terminal pane or window edge. */
const LABEL_GAP = 6;

function LabelProjector({
  sim,
  residents,
  labels,
  stage,
}: {
  sim: Sim;
  residents: Resident[];
  labels: RefObject<Map<string, HTMLDivElement>>;
  stage: Stage | null;
}) {
  const v = useMemo(() => new THREE.Vector3(), []);
  // Each tag's size, kept by an observer rather than read every frame.
  const sizes = useMemo(
    () => new WeakMap<Element, { width: number; height: number }>(),
    [],
  );
  const observer = useMemo(
    () =>
      new ResizeObserver((entries) => {
        for (const e of entries) {
          const box = e.borderBoxSize[0];
          sizes.set(e.target, {
            width: box?.inlineSize ?? 0,
            height: box?.blockSize ?? 0,
          });
        }
      }),
    [sizes],
  );
  useEffect(() => () => observer.disconnect(), [observer]);
  const sizeOf = (node: Element | null) => {
    if (!node) return { width: 0, height: 0 };
    if (!sizes.has(node)) {
      const el = node as HTMLElement;
      sizes.set(node, { width: el.offsetWidth, height: el.offsetHeight });
      observer.observe(node);
    }
    return sizes.get(node)!;
  };
  useFrame(({ camera, size }) => {
    const toPx = (x: number) => size.width / 2 + x * PIXELS_PER_UNIT;
    for (const r of residents) {
      const el = labels.current.get(r.id);
      const a = sim.agents.get(r.id);
      if (!el || !a) continue;
      const head =
        a.activity === "egg"
          ? 0.95
          : ANIMALS[r.animal].top * (r.depth > 0 ? YOUNG_HEAD : 1) + 0.1;
      const lift =
        a.activity === "pitch"
          ? STAGE_HEIGHT
          : a.activity === "seated"
            ? STAGE_HEIGHT + SEAT_LIFT
            : a.activity === "quilt"
              ? QUILT_LIFT
              : a.activity === "cheer"
                ? CHEER_LIFT
                : 0;
      const aloft = a.aloft * CHOSEN_HOVER * a.size;
      v.set(a.pos.x, head * a.size + 0.1 + lift + aloft, a.pos.z);
      v.project(camera);
      let x = ((v.x + 1) / 2) * size.width;
      const y =
        ((1 - v.y) / 2) * size.height -
        (a.activity === "queue" && (sim.lineIndex(r.id) ?? 0) % 2
          ? DOCK_STAGGER_PX
          : 0);
      // Kept on screen, and above all out from under the terminal pane: the
      // pane wins when a long tag can't have both.
      const clamp = (x: number, width: number) => {
        const half = width / 2 + LABEL_GAP;
        x = Math.min(Math.max(x, half), size.width - half);
        if (!stage) return x;
        return a.side === "left"
          ? Math.min(x, toPx(stage.panel.xMin) - half)
          : Math.max(x, toPx(stage.panel.xMax) + half);
      };
      const tag = Math.max(
        sizeOf(el.querySelector(".fun-pill")).width,
        sizeOf(el.querySelector(".fun-note")).width,
      );
      x = clamp(x, tag);
      // Its top left on a whole device pixel: a tag between pixels is drawn
      // blurred, which shows on a screen that isn't Retina.
      const box = sizeOf(el);
      const dpr = window.devicePixelRatio;
      const snap = (px: number) => (Math.round(px * dpr) / dpr).toFixed(2);
      el.style.transform = `translate(${snap(x - box.width / 2)}px, ${snap(y - box.height)}px)`;
      el.style.opacity = String(a.portalScale);
      // Indoors, its tag goes in with it.
      el.style.visibility = a.portalScale < 0.02 ? "hidden" : "";
      // Sitting about, a resident gets a smaller tag so more fit side by side.
      el.classList.toggle("compact", COMPACT.has(a.activity));
    }
  });
  return null;
}

function Label({
  resident,
  hovered,
  kin,
  register,
  handlers,
}: {
  resident: Resident;
  hovered: boolean;
  kin: boolean;
  register: (el: HTMLDivElement | null) => void;
  handlers: CharacterHandlers;
}) {
  const { mood, progress } = resident;
  const badge =
    mood === "needs_you"
      ? "!"
      : mood === "hatching"
        ? "…"
        : mood === "failed"
          ? "×"
          : null;
  const classes = ["fun-label", `mood-${mood}`];
  if (resident.selected) classes.push("selected");
  if (hovered) classes.push("hovered");
  if (kin) classes.push("kin");
  if (resident.depth > 0) classes.push("child");
  return (
    <div
      ref={register}
      className={classes.join(" ")}
      style={{ "--family": ANIMALS[resident.animal].accent } as CSSProperties}
      onClick={() => handlers.onSelect(resident.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        handlers.onMenu(resident.id, e.clientX, e.clientY);
      }}
      onMouseEnter={() => handlers.onHover(resident.id)}
      onMouseLeave={() => handlers.onHover(null)}
    >
      {badge && <span className="fun-badge">{badge}</span>}
      {resident.note && <span className="fun-note">{resident.note}</span>}
      <span className="fun-pill">
        <span className="fun-branch">{resident.branch}</span>
        {progress && progress.total > 0 && (
          <span className="fun-progress">
            {progress.done}/{progress.total}
          </span>
        )}
      </span>
    </div>
  );
}

/**
 * A hovered parent and everyone it handed work to, down the generations; empty
 * when what's hovered has no children.
 */
function familyOf(id: string | null, residents: Resident[]): Set<string> {
  const family = new Set<string>();
  if (!id) return family;
  const children = new Map<string, string[]>();
  for (const r of residents) {
    if (r.parentId) {
      children.set(r.parentId, [...(children.get(r.parentId) ?? []), r.id]);
    }
  }
  if (!children.has(id)) return family;
  const stack = [id];
  while (stack.length) {
    const next = stack.pop()!;
    if (family.has(next)) continue;
    family.add(next);
    stack.push(...(children.get(next) ?? []));
  }
  return family;
}

export default function FunWorld({
  workspaces,
  needsTurn,
  working,
  selectedId,
  onSelect,
  folders,
  onSetFolderCollapsed,
  menuActions,
  panelRef,
  onManualClock,
  weather: fixedWeather,
  environment: fixedEnvironment,
}: FunWorldProps) {
  const animals = useAnimals(workspaces);
  const stage = useStage(panelRef, folders.length);
  const environment = useToday(fixedEnvironment);
  const outdoors = OUTDOORS[environment];
  const forecast = useWeather(fixedWeather);
  const weather = outdoors ? forecast : CLEAR;
  const look = usePalette(environment);
  const palette = useMemo(() => weathered(look, weather), [look, weather]);
  const sim = useMemo(() => new Sim(), []);
  const labels = useRef(new Map<string, HTMLDivElement>());
  const voice = useRef<Voice>({ lastKeyAt: -Infinity });
  // Capture phase, since the terminal stops the keys it handles from bubbling.
  useEffect(() => {
    const heard = () => {
      voice.current.lastKeyAt = performance.now();
    };
    window.addEventListener("keydown", heard, true);
    return () => window.removeEventListener("keydown", heard, true);
  }, []);
  const [hovered, setHovered] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(
    null,
  );
  // Whether a menu was open when this click began: the menu closes itself on
  // mousedown, before the click lands, and that click shouldn't also deselect.
  const menuOpenAtDown = useRef(false);

  const menuFor = menu ? workspaces.find((w) => w.id === menu.id) : undefined;

  const live = useMemo(
    () =>
      toResidents({
        workspaces,
        needsTurn,
        working,
        selectedId,
        animals,
        folders,
      }),
    [workspaces, needsTurn, working, selectedId, animals, folders],
  );
  // By hand, a change of mood is wanted in the very next step.
  const residents = useSettled(live, onManualClock ? 0 : SETTLE_MS);

  // Layout first, so new residents spawn inside today's meadows.
  useLayoutEffect(() => {
    if (stage) sim.setLayout(stage.layout);
  }, [sim, stage]);
  // Deleted but still sailing off: drawn as they last were until they're gone.
  const [sailors, setSailors] = useState<Resident[]>([]);
  useLayoutEffect(() => {
    sim.sync(residents);
    const next = sim.sailors();
    setSailors((prev) => (prev.length || next.length ? next : prev));
  }, [sim, residents]);
  const onDeparted = useCallback(() => setSailors(sim.sailors()), [sim]);
  const shown = useMemo(() => [...residents, ...sailors], [residents, sailors]);
  const kin = useMemo(() => familyOf(hovered, shown), [hovered, shown]);

  useEffect(() => {
    document.body.style.cursor = hovered ? "pointer" : "";
    return () => {
      document.body.style.cursor = "";
    };
  }, [hovered]);

  const handlers: CharacterHandlers = useMemo(
    () => ({
      onSelect,
      onHover: setHovered,
      onMenu: (id, x, y) => setMenu({ id, x, y }),
    }),
    [onSelect],
  );

  return (
    <div
      className="fun-world"
      onPointerDownCapture={() => {
        menuOpenAtDown.current = menu !== null;
      }}
    >
      <Canvas
        orthographic
        flat
        frameloop="never"
        dpr={[1, 2]}
        camera={{ zoom: PIXELS_PER_UNIT, near: 0.1, far: 200 }}
        onPointerMissed={(e) => {
          setMenu(null);
          if (e.button === 0 && !menuOpenAtDown.current) onSelect(null);
        }}
      >
        <EnvironmentContext.Provider value={environment}>
          <color attach="background" args={[palette.ground]} />
          <Aim />
          <FrameDriver
            sim={sim}
            onDeparted={onDeparted}
            onManualClock={onManualClock}
          />
          <SimDriver
            sim={sim}
            onDeparted={onDeparted}
            manual={!!onManualClock}
          />
          {stage && (
            <Scenery
              stage={stage}
              palette={palette}
              pitching={residents.some(
                (r) =>
                  r.mood === "shipping" &&
                  (r.home === null || !stage.layout.houses[r.home]),
              )}
              sim={sim}
            />
          )}
          {stage && outdoors && (
            <WeatherLayer stage={stage} weather={weather} sky={palette.sky} />
          )}
          {shown.map((r) => (
            <Character
              key={r.id}
              resident={r}
              sim={sim}
              handlers={handlers}
              voice={voice.current}
              kin={kin.has(r.id)}
            />
          ))}
          {stage && (
            <Houses
              houses={stage.layout.houses}
              folders={folders}
              sim={sim}
              onToggle={(f) => onSetFolderCollapsed(f.id, !f.collapsed)}
            />
          )}
          <DivineBeam sim={sim} selectedId={selectedId} voice={voice.current} />
          <LabelProjector
            sim={sim}
            residents={shown}
            labels={labels}
            stage={stage}
          />
        </EnvironmentContext.Provider>
      </Canvas>
      <div className="fun-labels">
        {shown.map((r) => (
          <Label
            key={r.id}
            resident={r}
            hovered={hovered === r.id}
            kin={kin.has(r.id)}
            handlers={handlers}
            register={(el) => {
              if (el) labels.current.set(r.id, el);
              else labels.current.delete(r.id);
            }}
          />
        ))}
      </div>
      {menuFor && menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          workspace={menuFor}
          hasTurn={needsTurn(menuFor)}
          liveChildren={
            workspaces.filter((w) => w.parent === menuFor.id).length
          }
          onClose={() => setMenu(null)}
          {...menuActions}
        />
      )}
    </div>
  );
}
