import { useEffect, useLayoutEffect, useMemo, useState } from "react";
import { Canvas, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { ANIMAL_NAMES, type AnimalName } from "./animals";
import { Character, type CharacterHandlers } from "./Character";
import type { Resident } from "./residents";
import { MEADOW } from "./Scenery";
import { sizeForDepth, type Activity, type Agent, type Sim } from "./sim";
import { CAMERA_DISTANCE, PITCH } from "./stage";

/**
 * Every animal, grown up and as a child, in one pose: for designing them.
 * `fun-preview.html?gallery`, with `&pose=walk` (any activity), `&facing=`,
 * `&zoom=` and `&only=fox,cat`.
 * With `?shot`, `window.__fun` holds the clock and `pose(activity, facing)`.
 */

const ONLY = new URLSearchParams(location.search).get("only")?.split(",");
/** The animals shown, in the gallery's order. */
const SHOWN = ANIMAL_NAMES.filter((a) => !ONLY || ONLY.includes(a));
const COLUMNS = Math.min(5, SHOWN.length);
const CELL_X = 2.2;
const CELL_Z = 2.4;
/** Where a child stands beside its parent. */
const CHILD_X = 0.85;

const noop: CharacterHandlers = {
  onSelect: () => {},
  onHover: () => {},
  onMenu: () => {},
};

interface Pose {
  activity: Activity;
  facing: number;
}

function cell(i: number) {
  const col = i % COLUMNS;
  const row = Math.floor(i / COLUMNS);
  const rows = Math.ceil(SHOWN.length / COLUMNS);
  return {
    x: (col - (COLUMNS - 1) / 2) * CELL_X - CHILD_X / 2,
    z: (row - (rows - 1) / 2) * CELL_Z,
  };
}

/** Just enough of a sim for `Character` to read its agent and the time. */
function useGallerySim(pose: Pose) {
  const [clock] = useState(() => ({ now: 0 }));
  const sim = useMemo(() => {
    const agents = new Map<string, Agent>();
    SHOWN.forEach((name, i) => {
      const { x, z } = cell(i);
      [0, 1].forEach((depth) => {
        agents.set(`${name}-${depth}`, {
          id: `${name}-${depth}`,
          pos: { x: x + depth * CHILD_X, z },
          size: sizeForDepth(depth),
          portalScale: 1,
          phase: i * 0.7 + depth * 0.3,
          since: 0,
          aloft: 0,
          carrying: null,
          chore: "water",
        } as unknown as Agent);
      });
    });
    return {
      agents,
      time: () => clock.now,
      afloat: () => false,
    } as unknown as Sim;
  }, [clock]);
  for (const a of sim.agents.values()) {
    a.activity = pose.activity;
    a.facing = pose.facing;
  }
  return { sim, clock };
}

const residents: Resident[] = SHOWN.flatMap((name) =>
  [0, 1].map((depth) => ({
    id: `${name}-${depth}`,
    branch: name,
    mood: "idle" as const,
    parentId: depth ? `${name}-0` : null,
    depth,
    selected: false,
    animal: name,
    work: "typing" as const,
    progress: null,
    note: null,
    readyToMerge: false,
    quilts: false,
    home: null,
  })),
);

function Aim({
  onPlaced,
}: {
  onPlaced: (at: Map<AnimalName, { x: number; y: number }>) => void;
}) {
  const camera = useThree((s) => s.camera);
  const size = useThree((s) => s.size);
  useLayoutEffect(() => {
    camera.position.set(
      0,
      Math.sin(PITCH) * CAMERA_DISTANCE,
      Math.cos(PITCH) * CAMERA_DISTANCE,
    );
    camera.lookAt(0, 0, 0);
    camera.updateMatrixWorld();
    camera.updateProjectionMatrix();
    const v = new THREE.Vector3();
    onPlaced(
      new Map(
        SHOWN.map((name, i) => {
          const { x, z } = cell(i);
          v.set(x + CHILD_X / 2, 0, z + 0.45).project(camera);
          return [
            name,
            {
              x: ((v.x + 1) / 2) * size.width,
              y: ((1 - v.y) / 2) * size.height,
            },
          ];
        }),
      ),
    );
  }, [camera, size, onPlaced]);
  return null;
}

/** Draws on the display's refresh, or only when stepped by hand. */
function Driver({
  clock,
  manual,
  onPose,
}: {
  clock: { now: number };
  manual: boolean;
  onPose: (pose: Pose) => void;
}) {
  const advance = useThree((s) => s.advance);
  useEffect(() => {
    if (manual) {
      Object.assign(window, {
        __fun: {
          step: async (seconds: number) => {
            clock.now += seconds;
            await new Promise((r) => setTimeout(r, 0));
            advance(clock.now);
          },
          pose: (activity: Activity, facing = 0.35) =>
            onPose({ activity, facing }),
          ids: () => SHOWN,
        },
      });
      return;
    }
    let raf = 0;
    const tick = (now: number) => {
      raf = requestAnimationFrame(tick);
      clock.now = now / 1000;
      advance(clock.now);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [advance, clock, manual, onPose]);
  return null;
}

export default function Gallery() {
  const params = new URLSearchParams(location.search);
  const [pose, setPose] = useState<Pose>({
    activity: (params.get("pose") as Activity) ?? "pause",
    facing: Number(params.get("facing") ?? 0.35),
  });
  const { sim, clock } = useGallerySim(pose);
  const [names, setNames] = useState<Map<AnimalName, { x: number; y: number }>>(
    new Map(),
  );
  return (
    <div style={{ position: "fixed", inset: 0 }}>
      <Canvas
        orthographic
        flat
        frameloop="never"
        dpr={[1, 2]}
        camera={{
          zoom: Number(params.get("zoom") ?? 120),
          near: 0.1,
          far: 200,
        }}
      >
        <color attach="background" args={[MEADOW.ground]} />
        <hemisphereLight
          args={[MEADOW.sky, MEADOW.hemiGround, MEADOW.hemiIntensity]}
        />
        <directionalLight
          position={[6, 12, 8]}
          intensity={MEADOW.sunIntensity}
          color={MEADOW.sun}
        />
        <Aim onPlaced={setNames} />
        <Driver clock={clock} manual={params.has("shot")} onPose={setPose} />
        {residents.map((r) => (
          <Character
            key={r.id}
            resident={r}
            sim={sim}
            handlers={noop}
            voice={{ lastKeyAt: -Infinity }}
          />
        ))}
      </Canvas>
      {[...names].map(([name, at]) => (
        <div
          key={name}
          style={{
            position: "absolute",
            left: at.x,
            top: at.y,
            transform: "translateX(-50%)",
            font: "600 13px system-ui",
            color: "#2a3a1e",
          }}
        >
          {name}
        </div>
      ))}
    </div>
  );
}
