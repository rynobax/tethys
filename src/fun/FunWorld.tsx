import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import * as api from "../ipc/commands";
import { useAppEvent } from "../ipc/events";
import { useTheme } from "../theme";
import type { SessionInfo, Workspace, WorkspaceId } from "../types";
import {
  Character,
  DIMS,
  HAT_ROOM,
  SEAT_LIFT,
  type Anchors,
  type CharacterHandlers,
  type Voice,
} from "./Character";
import { Leash } from "./Leash";
import { toResidents, type Resident } from "./residents";
import { DUSK, MEADOW, Scenery, type Environment } from "./Scenery";
import { Sim, type Activity } from "./sim";
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
  sessions: Map<WorkspaceId, SessionInfo | null>;
  needsTurn: (w: Workspace) => boolean;
  working: (w: Workspace) => boolean;
  selectedId: WorkspaceId | null;
  onSelect: (id: WorkspaceId) => void;
  /** The terminal panel; the meadows are whatever it leaves either side. */
  panelRef: RefObject<HTMLElement | null>;
}

function useAvatars(): Map<WorkspaceId, unknown> {
  const [avatars, setAvatars] = useState<Map<WorkspaceId, unknown>>(new Map());
  const refresh = useCallback(() => {
    api
      .listAvatars()
      .then((all) =>
        setAvatars(new Map(Object.entries(all).map(([id, a]) => [id, a.spec]))),
      )
      .catch((e) => console.error("list_avatars failed:", e));
  }, []);
  useEffect(() => {
    refresh();
    // Workspaces from before fun mode existed get designed on first visit.
    api
      .ensureAvatars()
      .catch((e) => console.error("ensure_avatars failed:", e));
  }, [refresh]);
  useAppEvent("avatar:changed", refresh);
  return avatars;
}

function useStage(panelRef: RefObject<HTMLElement | null>): Stage | null {
  const [stage, setStage] = useState<Stage | null>(null);
  useLayoutEffect(() => {
    const measure = () => {
      const panel = panelRef.current?.getBoundingClientRect();
      if (!panel) return;
      setStage(
        computeStage(window.innerWidth, window.innerHeight, {
          left: panel.left,
          right: panel.right,
        }),
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (panelRef.current) observer.observe(panelRef.current);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [panelRef]);
  return stage;
}

function useEnvironment(): Environment {
  const theme = useTheme();
  return useMemo(() => {
    const bg = theme?.colors.background;
    if (bg) {
      const { r, g, b } = new THREE.Color(bg);
      return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.35 ? DUSK : MEADOW;
    }
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? DUSK
      : MEADOW;
  }, [theme]);
}

/**
 * Drives every frame off the display's own refresh, capped at `MAX_FPS`, and
 * at full rate whether the window is focused or not: the world is for
 * glancing at while working somewhere else. The browser stops calling it
 * while the window is hidden.
 */
function FrameDriver() {
  const advance = useThree((s) => s.advance);
  useEffect(() => {
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
  }, [advance]);
  return null;
}

function SimDriver({ sim }: { sim: Sim }) {
  // Mounted before every Character, so they all pose the same step.
  useFrame((_, delta) => sim.step(Math.min(delta, 0.1)));
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
const COMPACT: ReadonlySet<Activity> = new Set(["seated", "lounge", "sleep"]);

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

function LabelProjector({
  sim,
  residents,
  labels,
}: {
  sim: Sim;
  residents: Resident[];
  labels: RefObject<Map<string, HTMLDivElement>>;
}) {
  const v = useMemo(() => new THREE.Vector3(), []);
  useFrame(({ camera, size }) => {
    for (const r of residents) {
      const el = labels.current.get(r.id);
      const a = sim.agents.get(r.id);
      if (!el || !a) continue;
      const head =
        a.activity === "egg" ? 0.95 : DIMS[r.spec.body].top + HAT_ROOM;
      const lift =
        a.activity === "pitch"
          ? STAGE_HEIGHT
          : a.activity === "seated"
            ? STAGE_HEIGHT + SEAT_LIFT
            : 0;
      v.set(a.pos.x, head * a.size + 0.1 + lift, a.pos.z).project(camera);
      const x = ((v.x + 1) / 2) * size.width;
      const y = ((1 - v.y) / 2) * size.height;
      el.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) translate(-50%, -100%)`;
      el.style.opacity = String(a.portalScale);
      // Sitting about, a resident gets a smaller tag so more fit side by side.
      el.classList.toggle("compact", COMPACT.has(a.activity));
    }
  });
  return null;
}

function Label({
  resident,
  hovered,
  register,
  handlers,
}: {
  resident: Resident;
  hovered: boolean;
  register: (el: HTMLDivElement | null) => void;
  handlers: CharacterHandlers;
}) {
  const { mood, spec, progress } = resident;
  const badge =
    mood === "needs_you"
      ? "!"
      : mood === "asleep"
        ? "z"
        : mood === "hatching"
          ? "…"
          : mood === "failed"
            ? "×"
            : null;
  const classes = ["fun-label", `mood-${mood}`];
  if (resident.selected) classes.push("selected");
  if (hovered) classes.push("hovered");
  if (resident.depth > 0) classes.push("child");
  return (
    <div
      ref={register}
      className={classes.join(" ")}
      onClick={() => handlers.onSelect(resident.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        handlers.onMenu(resident.id, e.clientX, e.clientY);
      }}
      onMouseEnter={() => handlers.onHover(resident.id)}
      onMouseLeave={() => handlers.onHover(null)}
    >
      {badge && <span className="fun-badge">{badge}</span>}
      {hovered && resident.designed && (
        <span className="fun-card">
          <span className="fun-card-name">{spec.name}</span>
          {spec.tagline && (
            <span className="fun-card-tagline">{spec.tagline}</span>
          )}
        </span>
      )}
      {resident.note && <span className="fun-note">{resident.note}</span>}
      <span className="fun-pill">
        {resident.branch}
        {progress && progress.total > 0 && (
          <span className="fun-progress">
            {progress.done}/{progress.total}
          </span>
        )}
      </span>
    </div>
  );
}

function ResidentMenu({
  x,
  y,
  onOpen,
  onRedesign,
  onClose,
}: {
  x: number;
  y: number;
  onOpen: () => void;
  onRedesign: () => void;
  onClose: () => void;
}) {
  useEffect(() => {
    const close = () => onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
    };
  }, [onClose]);
  const act = (fn: () => void) => () => {
    fn();
    onClose();
  };
  return (
    <div
      className="context-menu fun-menu"
      style={{ left: x, top: y }}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <button type="button" onClick={act(onOpen)}>
        Open
      </button>
      <button type="button" onClick={act(onRedesign)}>
        Redesign avatar
      </button>
    </div>
  );
}

export default function FunWorld({
  workspaces,
  sessions,
  needsTurn,
  working,
  selectedId,
  onSelect,
  panelRef,
}: FunWorldProps) {
  const avatars = useAvatars();
  const stage = useStage(panelRef);
  const env = useEnvironment();
  const sim = useMemo(() => new Sim(), []);
  const labels = useRef(new Map<string, HTMLDivElement>());
  const anchors = useRef<Anchors>(new Map());
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

  const residents = useMemo(
    () =>
      toResidents({
        workspaces,
        sessions,
        needsTurn,
        working,
        selectedId,
        avatars,
      }),
    [workspaces, sessions, needsTurn, working, selectedId, avatars],
  );

  // Layout first, so new residents spawn inside today's meadows.
  useLayoutEffect(() => {
    if (stage) sim.setLayout(stage.layout);
  }, [sim, stage]);
  useLayoutEffect(() => sim.sync(residents), [sim, residents]);

  useEffect(() => {
    document.body.style.cursor = hovered ? "pointer" : "";
    return () => {
      document.body.style.cursor = "";
    };
  }, [hovered]);

  // A child's collar and leash are in its parent's colours.
  const byId = useMemo(
    () => new Map(residents.map((r) => [r.id, r])),
    [residents],
  );
  const leashColor = (r: Resident) =>
    r.parentId ? (byId.get(r.parentId)?.spec.accent_color ?? null) : null;

  const handlers: CharacterHandlers = useMemo(
    () => ({
      onSelect,
      onHover: setHovered,
      onMenu: (id, x, y) => setMenu({ id, x, y }),
    }),
    [onSelect],
  );

  return (
    <div className="fun-world">
      <Canvas
        orthographic
        flat
        frameloop="never"
        dpr={[1, 2]}
        camera={{ zoom: PIXELS_PER_UNIT, near: 0.1, far: 200 }}
        onPointerMissed={() => setMenu(null)}
      >
        <color attach="background" args={[env.ground]} />
        <Aim />
        <FrameDriver />
        <SimDriver sim={sim} />
        {stage && (
          <Scenery
            stage={stage}
            env={env}
            pitching={residents.some((r) => r.mood === "shipping")}
            sim={sim}
          />
        )}
        {residents.map((r) => (
          <Character
            key={r.id}
            resident={r}
            sim={sim}
            handlers={handlers}
            anchors={anchors.current}
            collarColor={leashColor(r)}
            voice={voice.current}
          />
        ))}
        {/* After every Character, so both ends have been posed this frame. */}
        {residents.map((r) => {
          const color = leashColor(r);
          return r.parentId && color ? (
            <Leash
              key={r.id}
              parentId={r.parentId}
              childId={r.id}
              sim={sim}
              anchors={anchors.current}
              color={color}
            />
          ) : null;
        })}
        <DivineBeam sim={sim} selectedId={selectedId} voice={voice.current} />
        <LabelProjector sim={sim} residents={residents} labels={labels} />
      </Canvas>
      <div className="fun-labels">
        {residents.map((r) => (
          <Label
            key={r.id}
            resident={r}
            hovered={hovered === r.id}
            handlers={handlers}
            register={(el) => {
              if (el) labels.current.set(r.id, el);
              else labels.current.delete(r.id);
            }}
          />
        ))}
      </div>
      {menu && (
        <ResidentMenu
          x={menu.x}
          y={menu.y}
          onOpen={() => onSelect(menu.id)}
          onRedesign={() =>
            api
              .rerollAvatar(menu.id)
              .catch((e) => console.error("reroll_avatar failed:", e))
          }
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}
