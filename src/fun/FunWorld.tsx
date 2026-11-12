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
import { Character, DIMS, HAT_ROOM, type CharacterHandlers } from "./Character";
import { toResidents, type Resident } from "./residents";
import { DUSK, MEADOW, Scenery, type Environment } from "./Scenery";
import { Sim } from "./sim";
import {
  CAMERA_DISTANCE,
  PITCH,
  PIXELS_PER_UNIT,
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
      advance(now);
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
      v.set(a.pos.x, head * a.size + 0.1, a.pos.z).project(camera);
      const x = ((v.x + 1) / 2) * size.width;
      const y = ((1 - v.y) / 2) * size.height;
      el.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) translate(-50%, -100%)`;
      el.style.opacity = String(a.portalScale);
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
        {stage && <Scenery stage={stage} env={env} />}
        {residents.map((r) => (
          <Character key={r.id} resident={r} sim={sim} handlers={handlers} />
        ))}
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
