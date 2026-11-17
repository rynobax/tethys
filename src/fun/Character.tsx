import { useRef } from "react";
import { useFrame, type ThreeEvent } from "@react-three/fiber";
import * as THREE from "three";
import type { Resident } from "./residents";
import type { Agent, Sim } from "./sim";
import { useCropMaterials } from "./Garden";
import { BOAT_SCALE, Boat, Drink, FloatRing } from "./Scenery";
import { ANIMALS } from "./animals";
import { Mat, RIG, YOUNG_HEAD } from "./animals/rig";
import { hashString, type Work } from "./spec";
import { STAGE_HEIGHT } from "./stage";

/** Where the cork sits on a champagne bottle held from below. */
const BOTTLE_TOP = 0.37;

/** A bottle of champagne, its cork and the spray from popping it. */
function Champagne({
  cork,
  spray,
}: {
  cork: React.RefObject<THREE.Mesh | null>;
  spray: React.RefObject<THREE.Group | null>;
}) {
  const glass = "#1f5e3a";
  const foil = "#e8c24a";
  return (
    <group>
      <mesh position-y={0.1}>
        <cylinderGeometry args={[0.06, 0.06, 0.2, 10]} />
        <meshStandardMaterial color={glass} />
      </mesh>
      <mesh position-y={0.1}>
        <cylinderGeometry args={[0.062, 0.062, 0.07, 10]} />
        <meshStandardMaterial color="#fff8e6" />
      </mesh>
      <mesh position-y={0.235}>
        <cylinderGeometry args={[0.024, 0.06, 0.07, 10]} />
        <meshStandardMaterial color={glass} />
      </mesh>
      <mesh position-y={0.31}>
        <cylinderGeometry args={[0.026, 0.026, 0.09, 8]} />
        <meshStandardMaterial color={foil} />
      </mesh>
      <mesh ref={cork} position-y={BOTTLE_TOP}>
        <cylinderGeometry args={[0.022, 0.018, 0.045, 8]} />
        <meshStandardMaterial color="#d9b98a" />
      </mesh>
      <group ref={spray} visible={false}>
        {Array.from({ length: 9 }, (_, i) => (
          <mesh key={i}>
            <sphereGeometry args={[0.028, 6, 5]} />
            <meshBasicMaterial color={i % 3 ? "#fffbe8" : "#ffe9a0"} />
          </mesh>
        ))}
      </group>
    </group>
  );
}

/** Stands in for the eyes of a head that hasn't any to blink. */
const NO_EYES = new THREE.Object3D();

/** Typing reaches the chosen one for this long after each keystroke. */
const VOICE_HOLD_MS = 900;
/** How far above the stage a seated resident's root sits, on its chair. */
export const SEAT_LIFT = 0.1;
/** Likewise in a rocking chair on the porch, and stood up on its seat. */
export const QUILT_LIFT = 0.12;
export const CHEER_LIFT = 0.2;
/** Likewise on a pool lounger's cushion. */
const LOUNGER_LIFT = 0.19;
/** How far back a lounging resident reclines. */
const RECLINE = -1.0;

/** How high the chosen one floats in its light, in its own units. */
export const CHOSEN_HOVER = 0.75;
/** Leaning back that far, chest to the sky, about the middle of its body. */
const CHOSEN_LEAN = -1.15;
const CHOSEN_PIVOT = 0.4;

export interface Voice {
  lastKeyAt: number;
}

/** Set up in front of the resident while it works. */
function Station({ work, accent }: { work: Work; accent: string }) {
  switch (work) {
    case "typing":
      return (
        <group>
          <mesh position-y={0.28}>
            <boxGeometry args={[0.6, 0.04, 0.32]} />
            <Mat color="#c08a5b" />
          </mesh>
          {[-1, 1].map((s) => (
            <mesh key={s} position={[s * 0.26, 0.13, 0]}>
              <boxGeometry args={[0.04, 0.26, 0.26]} />
              <Mat color="#a8754a" />
            </mesh>
          ))}
          <mesh position={[0, 0.31, -0.02]}>
            <boxGeometry args={[0.32, 0.02, 0.2]} />
            <Mat color="#d0d4da" />
          </mesh>
          <mesh position={[0, 0.41, 0.08]} rotation-x={0.35}>
            <boxGeometry args={[0.32, 0.2, 0.02]} />
            <Mat color="#d0d4da" />
          </mesh>
          <mesh position={[0, 0.41, 0.094]} rotation-x={0.35}>
            <planeGeometry args={[0.27, 0.16]} />
            <meshBasicMaterial color={accent} side={THREE.DoubleSide} />
          </mesh>
        </group>
      );
    case "hammering":
      return (
        <group position-y={0}>
          <mesh position-y={0.1}>
            <boxGeometry args={[0.18, 0.2, 0.18]} />
            <Mat color="#4a4f57" />
          </mesh>
          <mesh position-y={0.24}>
            <boxGeometry args={[0.38, 0.08, 0.2]} />
            <Mat color="#5c626b" />
          </mesh>
          <mesh position={[0.05, 0.31, 0]}>
            <boxGeometry args={[0.14, 0.05, 0.1]} />
            <Mat color="#ff9f43" emissive={0.7} />
          </mesh>
        </group>
      );
    case "stirring":
      return (
        <group>
          <mesh position-y={0.15}>
            <cylinderGeometry args={[0.24, 0.18, 0.3, 10]} />
            <Mat color="#2f3237" />
          </mesh>
          <mesh position-y={0.301} rotation-x={-Math.PI / 2}>
            <circleGeometry args={[0.21, 12]} />
            <meshBasicMaterial color={accent} />
          </mesh>
        </group>
      );
    case "reading":
    case "sweeping":
      return null;
  }
}

/** What the busy hand holds while working, if anything. */
function Tool({ work }: { work: Work }) {
  switch (work) {
    case "hammering":
      return (
        <group rotation-x={Math.PI / 2}>
          <mesh position-y={0.08}>
            <cylinderGeometry args={[0.02, 0.02, 0.26, 5]} />
            <Mat color="#9a6b47" />
          </mesh>
          <mesh position-y={0.22}>
            <boxGeometry args={[0.16, 0.07, 0.07]} />
            <Mat color="#7a7f88" />
          </mesh>
        </group>
      );
    case "stirring":
      return (
        <mesh position-y={-0.12}>
          <cylinderGeometry args={[0.018, 0.018, 0.4, 5]} />
          <Mat color="#c08a5b" />
        </mesh>
      );
    case "sweeping":
      return (
        <group position-y={-0.15} rotation-x={0.3}>
          <mesh>
            <cylinderGeometry args={[0.018, 0.018, 0.6, 5]} />
            <Mat color="#c08a5b" />
          </mesh>
          <mesh position-y={-0.33}>
            <coneGeometry args={[0.1, 0.16, 6]} />
            <Mat color="#e6c36a" />
          </mesh>
        </group>
      );
    case "typing":
    case "reading":
      return null;
  }
}

export interface CharacterHandlers {
  onSelect: (id: string) => void;
  onHover: (id: string | null) => void;
  onMenu: (id: string, x: number, y: number) => void;
}

/** One resident, posed every frame from its agent in the sim. */
export function Character({
  resident,
  sim,
  handlers,
  voice,
  kin,
}: {
  resident: Resident;
  sim: Sim;
  handlers: CharacterHandlers;
  /** In the hovered parent's family: ringed in its colour. */
  kin: boolean;
  /** When Ryan last typed, which the chosen one hears. */
  voice: Voice;
}) {
  const design = ANIMALS[resident.animal];
  const work = resident.work;
  const root = useRef<THREE.Group>(null);
  const figure = useRef<THREE.Group>(null);
  const body = useRef<THREE.Group>(null);
  const eyes = useRef<THREE.Group>(null);
  const armL = useRef<THREE.Group>(null);
  const armR = useRef<THREE.Group>(null);
  const footL = useRef<THREE.Group>(null);
  const footR = useRef<THREE.Group>(null);
  const tail = useRef<THREE.Group>(null);
  const egg = useRef<THREE.Group>(null);
  const station = useRef<THREE.Group>(null);
  const tool = useRef<THREE.Group>(null);
  const book = useRef<THREE.Group>(null);
  const parcel = useRef<THREE.Group>(null);
  const pedestal = useRef<THREE.Group>(null);
  const crate = useRef<THREE.Group>(null);
  const produce = useRef<THREE.Group>(null);
  const can = useRef<THREE.Group>(null);
  const drops = useRef<THREE.Group>(null);
  const cropMaterials = useCropMaterials();
  const drink = useRef<THREE.Group>(null);
  const bottle = useRef<THREE.Group>(null);
  const cork = useRef<THREE.Mesh>(null);
  const spray = useRef<THREE.Group>(null);
  const boat = useRef<THREE.Group>(null);
  const floatRing = useRef<THREE.Group>(null);
  const ring = useRef<THREE.Mesh>(null);
  const kinRing = useRef<THREE.Mesh>(null);
  const shadow = useRef<THREE.Mesh>(null);
  /** How far up its root stands, on a stage, chair or log. */
  const lift = useRef(0);
  const sparkles = useRef<THREE.Group>(null);
  /** 0..1, how hard the chosen one is listening to the voice from above. */
  const listen = useRef(0);
  const mood = resident.mood;

  useFrame((_, delta) => {
    const a: Agent | undefined = sim.agents.get(resident.id);
    if (!a || !root.current) return;
    const t = sim.time() + a.phase;
    const local = sim.time() - a.since;

    // Chosen, it rises from wherever it was standing or sitting.
    if (a.activity !== "chosen") {
      lift.current =
        a.activity === "pitch"
          ? STAGE_HEIGHT
          : a.activity === "seated"
            ? STAGE_HEIGHT + SEAT_LIFT
            : a.activity === "lounge"
              ? LOUNGER_LIFT
              : a.activity === "quilt"
                ? QUILT_LIFT
                : a.activity === "cheer"
                  ? CHEER_LIFT
                  : 0;
    }
    root.current.position.set(a.pos.x, lift.current, a.pos.z);
    root.current.rotation.y =
      a.facing +
      (a.activity === "portal_out" || a.activity === "portal_in"
        ? (1 - a.portalScale) * Math.PI * 3
        : 0);
    root.current.scale.setScalar(a.size * Math.max(0.001, a.portalScale));

    const isEgg = a.activity === "egg";
    egg.current!.visible = isEgg;
    figure.current!.visible = !isEgg;
    // Asleep it lies on its back, face to the sky; otherwise upright.
    const lying = a.activity === "sleep";
    const fig = figure.current!;
    fig.rotation.x = lying ? -1.45 : a.activity === "lounge" ? RECLINE : 0;
    fig.position.set(0, lying ? 0.3 : 0, 0);
    // Asleep on a ring in the pool: bobbing, and turning slowly on the water.
    const afloat = sim.afloat(resident.id);
    floatRing.current!.visible = afloat;
    if (afloat) {
      root.current.position.y = 0.03 + Math.sin(t * 1.3) * 0.02;
      root.current.rotation.y += Math.sin(t * 0.13) * 0.6;
      root.current.rotation.z = Math.sin(t * 1.1) * 0.04;
    } else {
      root.current.rotation.z = 0;
    }
    // Off across the lake, sat in its boat as it rocks on the water.
    const sailing = a.activity === "sail";
    boat.current!.visible = sailing;
    // The moored boat's size, whatever size its sailor is.
    boat.current!.scale.setScalar(BOAT_SCALE / a.size);
    if (sailing) {
      root.current.position.y = 0.02 + Math.sin(t * 1.6) * 0.02;
      root.current.rotation.z = Math.sin(t * 1.2) * 0.05;
      fig.position.y = 0.16;
    }
    const f = a.aloft;
    shadow.current!.scale.setScalar(1 - 0.45 * f);
    const sp = sparkles.current!;
    sp.visible = f > 0.02;
    if (sp.visible) {
      sp.position.y = (CHOSEN_HOVER + 0.35) * f;
      sp.rotation.y = t * 1.4;
      sp.scale.setScalar(f);
    }
    if (ring.current) {
      ring.current.visible = resident.selected;
      ring.current.scale.setScalar(1 + Math.sin(t * 3) * 0.05);
    }
    if (kinRing.current) {
      kinRing.current.visible = kin;
      kinRing.current.scale.setScalar(1 + Math.sin(t * 5) * 0.08);
    }
    // Typing is the voice from above; only the chosen one hears it.
    const hearing =
      resident.selected && performance.now() - voice.lastKeyAt < VOICE_HOLD_MS;
    listen.current +=
      ((hearing ? 1 : 0) - listen.current) *
      Math.min(1, delta * (hearing ? 10 : 2.5));
    const k = listen.current;

    if (isEgg) {
      const e = egg.current!;
      e.position.y = k * 0.25;
      if (k > 0.01) {
        e.rotation.z = Math.sin(t * 28) * 0.12 * k;
      } else if (mood === "hatching") {
        const burst = Math.sin(t * 1.3) > 0.2 ? 1 : 0.15;
        e.rotation.z = Math.sin(t * 16) * 0.16 * burst;
      } else if (mood === "failed") {
        e.rotation.z = 0.4;
      } else {
        e.rotation.z = Math.sin(t * 2) * 0.04;
      }
      return;
    }

    const working = a.activity === "work";
    const pitching = a.activity === "pitch";
    // Nothing in its hands while it's chosen: they're flung wide.
    const chosen = a.activity === "chosen";
    // Carried to the stage, then set on its pedestal to be pitched.
    const shipping = mood === "shipping" && !pitching && !chosen;
    station.current!.visible = working;
    tool.current!.visible =
      working &&
      (work === "hammering" || work === "stirring" || work === "sweeping");
    book.current!.visible = working && work === "reading";
    parcel.current!.visible = shipping && !working;
    const carrying = a.carrying !== null && !working && !chosen;
    crate.current!.visible = carrying && !shipping;
    if (a.carrying !== null) {
      for (const m of produce.current!.children as THREE.Mesh[]) {
        m.material = cropMaterials[a.carrying];
      }
    }
    const gardening = a.activity === "garden";
    const watering = gardening && a.chore === "water";
    can.current!.visible = watering;
    drops.current!.visible = watering;
    if (watering) {
      drops.current!.children.forEach((m, i) => {
        const p = (t * 1.8 + i / drops.current!.children.length) % 1;
        m.position.set(Math.sin(i * 2.3) * 0.06, 0.4 - p * 0.36, 0);
      });
    }
    // A drink in hand on the lounger, sipped now and then.
    const lounging = a.activity === "lounge";
    // Champagne once its PRs are ready to merge, opened once they're merged.
    const champagne = resident.readyToMerge || mood === "shipped";
    const popping = a.activity === "pop";
    bottle.current!.visible = champagne;
    drink.current!.visible = lounging && !champagne;
    if (champagne) {
      // The cork shoots up and off in the first moment of popping.
      const c = cork.current!;
      const e = local;
      const corked = mood !== "shipped";
      c.visible = corked || (popping && e < 1.4);
      c.position.set(
        popping ? e * 0.7 : 0,
        BOTTLE_TOP + (popping ? e * 2.6 - e * e * 2.2 : 0),
        0,
      );
      c.rotation.z = popping ? e * 9 : 0;
      const sp = spray.current!;
      sp.visible = popping;
      if (popping) {
        sp.children.forEach((m, i) => {
          const p = (local * 1.6 + i / sp.children.length) % 1;
          const angle = i * 2.4;
          m.position.set(
            Math.sin(angle) * p * 0.28 + p * 0.25,
            BOTTLE_TOP + p * 0.75 - p * p * 0.55,
            Math.cos(angle) * p * 0.2,
          );
          m.scale.setScalar(1.2 - p);
        });
      }
    }
    const sip = lounging ? Math.max(0, Math.sin(t * 0.45) - 0.75) * 4 : 0;
    const handsBusy = working || shipping || carrying;
    pedestal.current!.visible = pitching;

    const b = body.current!;
    const L = armL.current!;
    const R = armR.current!;
    b.position.y = 0;
    b.rotation.set(0, 0, 0);
    b.scale.set(1, 1, 1);
    L.rotation.set(0, 0, -0.12);
    R.rotation.set(0, 0, 0.12);
    footL.current!.position.set(-RIG.foot.x, RIG.foot.y, RIG.foot.z);
    footR.current!.position.set(RIG.foot.x, RIG.foot.y, RIG.foot.z);
    // A tail sways, and wags quicker on the move.
    if (tail.current) {
      tail.current.rotation.y =
        Math.sin(t * (a.activity === "walk" ? 9 : 2.2)) * 0.25;
    }

    // Blink every few seconds; asleep keeps them shut.
    const ey = eyes.current ?? NO_EYES;
    const blink = t % 4.2 < 0.12;
    ey.scale.y = a.activity === "sleep" ? 0.12 : blink ? 0.15 : 1;

    switch (a.activity) {
      case "walk": {
        const s = Math.sin(t * 11);
        b.position.y = Math.abs(s) * 0.06;
        b.rotation.z = s * 0.05;
        if (!handsBusy) {
          L.rotation.x = s * 0.6;
          R.rotation.x = -s * 0.6;
        }
        footL.current!.position.z = 0.03 + s * 0.09;
        footR.current!.position.z = 0.03 - s * 0.09;
        footL.current!.position.y = 0.05 + Math.max(0, s) * 0.05;
        footR.current!.position.y = 0.05 + Math.max(0, -s) * 0.05;
        break;
      }
      case "pause":
      case "queue":
        b.scale.y = 1 + Math.sin(t * 2.2) * 0.02;
        b.rotation.y = Math.sin(t * 0.7) * 0.3;
        if (mood === "shipped" && Math.sin(t * 1.7) > 0.85) {
          b.position.y = Math.abs(Math.sin(t * 9)) * 0.12;
          L.rotation.z = -2.5;
          R.rotation.z = 2.5;
        }
        break;
      case "attention":
        b.position.y = Math.abs(Math.sin(t * 4.5)) * 0.1;
        R.rotation.z = 2.75 + Math.sin(t * 9) * 0.28;
        break;
      case "sleep":
        b.scale.y = 0.95 + Math.sin(t * 1.4) * 0.03;
        break;
      case "lounge": {
        // Reclined, one hand behind its head, the other holding its drink
        // and bringing it up for a sip now and then; feet idly waggling.
        const waggle = Math.sin(t * 1.8) * 0.04;
        footL.current!.position.set(-0.11, 0.06, 0.12 + waggle);
        footR.current!.position.set(0.11, 0.06, 0.12 - waggle);
        b.rotation.x = 0.15;
        L.rotation.set(-0.3, 0, -2.6);
        R.rotation.x = -0.75 - sip * 1.05;
        R.rotation.z = 0.15 + sip * 0.5;
        // Kept upright in the world, tipped towards its mouth for a sip.
        drink.current!.rotation.x = -(R.rotation.x + RECLINE) - sip * 0.7;
        break;
      }
      case "board":
        // Scribbling at the board, back to the viewer.
        R.rotation.x = -2.1 + Math.sin(t * 9) * 0.15;
        R.rotation.z = 0.25 + Math.sin(t * 6) * 0.25;
        b.rotation.z = Math.sin(t * 3) * 0.04;
        break;
      case "work":
        poseWork(work, t, local, b, L, R);
        break;
      case "garden":
        if (watering) {
          // The can held out over the plot, tipped to pour.
          R.rotation.x = -0.9;
          R.rotation.z = 0.55;
          b.rotation.y = Math.sin(t * 1.5) * 0.1;
        } else {
          // Bent over the plot, both hands busy in the soil.
          const dig = Math.sin(t * 8);
          b.position.y = -0.04;
          b.scale.y = 0.93;
          b.rotation.x = 0.3;
          L.rotation.x = -1.3 + dig * 0.3;
          R.rotation.x = -1.3 - dig * 0.3;
        }
        break;
      case "seated": {
        // Feet dangling off the chair, swinging, and a look round now and then.
        const swing = Math.sin(t * 3.2) * 0.07;
        footL.current!.position.set(-0.13, 0.12, 0.24 + swing);
        footR.current!.position.set(0.13, 0.12, 0.24 - swing);
        b.rotation.x = -0.08;
        b.rotation.y = Math.sin(t * 0.45) > 0.6 ? Math.sin(t * 0.9) * 0.5 : 0;
        break;
      }
      case "quilt": {
        // Rocking in its chair, feet swinging clear of the porch, leaning
        // over the frame to stitch: one hand holds the cloth, the other
        // draws the needle up and through.
        const rock = Math.sin(t * 1.3);
        footL.current!.position.set(-0.12, 0.1, 0.2 + rock * 0.03);
        footR.current!.position.set(0.12, 0.1, 0.2 + rock * 0.03);
        b.rotation.x = 0.12 + rock * 0.06;
        L.rotation.x = -1.05;
        L.rotation.z = 0.3;
        const pull = Math.max(0, Math.sin(t * 2.4));
        R.rotation.x = -1.1 + pull * 0.6;
        R.rotation.z = -0.25 - pull * 0.35;
        break;
      }
      case "cheer":
        // Up on its chair, hopping, both arms in the air.
        b.position.y = Math.abs(Math.sin(t * 7)) * 0.1;
        L.rotation.z = -2.6 + Math.sin(t * 9) * 0.2;
        R.rotation.z = 2.6 - Math.sin(t * 9) * 0.2;
        break;
      case "pop":
        // Bouncing, the bottle held high and spraying, the other arm cheering.
        b.position.y = Math.abs(Math.sin(t * 6)) * 0.08;
        R.rotation.z = 2.3;
        R.rotation.x = -0.3;
        L.rotation.z = -2.6 + Math.sin(t * 8) * 0.2;
        break;
      case "sail": {
        // Sat in the boat, feet up, waving goodbye with the bottle.
        footL.current!.position.set(-0.11, 0.08, 0.2);
        footR.current!.position.set(0.11, 0.08, 0.2);
        R.rotation.z = 2.5 + Math.sin(t * 5) * 0.35;
        L.rotation.x = -0.4;
        break;
      }
      case "pitch":
        // One hand sweeping up at the crowd, the other presenting the parcel.
        b.position.y = Math.abs(Math.sin(t * 2.6)) * 0.05;
        b.rotation.y = Math.sin(t * 1.3) * 0.25;
        L.rotation.z = -1.9 - Math.sin(t * 2.6) * 0.5;
        R.rotation.x = -1.1 + Math.sin(t * 4.3) * 0.3;
        R.rotation.z = 0.5;
        break;
    }
    if (handsBusy && a.activity !== "work" && a.activity !== "attention") {
      L.rotation.x = -1.15;
      R.rotation.x = -1.15;
      L.rotation.z = 0.15;
      R.rotation.z = -0.15;
    }

    // Chosen: lifted off the ground into its light, chest to the sky, arms
    // flung wide, eyes shut in bliss and feet dangling, whatever it was doing.
    if (f > 0.001) {
      fig.rotation.x += (CHOSEN_LEAN - fig.rotation.x) * f;
      fig.position.y +=
        (CHOSEN_HOVER +
          Math.sin(t * 1.6) * 0.05 +
          CHOSEN_PIVOT * (1 - Math.cos(CHOSEN_LEAN)) -
          fig.position.y) *
        f;
      // About the middle of the body, so it stays in the middle of the beam.
      fig.position.z = -CHOSEN_PIVOT * Math.sin(CHOSEN_LEAN) * f;
      b.rotation.y += (Math.sin(t * 0.7) * 0.3 - b.rotation.y) * f;
      b.rotation.z += Math.sin(t * 1.1) * 0.05 * f;
      const sweep = Math.sin(t * 1.6) * 0.15;
      L.rotation.x *= 1 - f;
      R.rotation.x *= 1 - f;
      L.rotation.z += (-1.75 - sweep - L.rotation.z) * f;
      R.rotation.z += (1.75 + sweep - R.rotation.z) * f;
      const dangle = Math.sin(t * 2.2) * 0.04 * f;
      footL.current!.position.y -= 0.03 * f;
      footR.current!.position.y -= 0.03 * f;
      footL.current!.position.z += dangle;
      footR.current!.position.z -= dangle;
      ey.scale.y += (0.15 - ey.scale.y) * f;
    }

    // Hearing the voice: rise higher, turn to the viewer, look up in awe with
    // arms raised and eyes wide open, trembling a little.
    if (k > 0.001) {
      fig.position.y += k * 0.3 + Math.sin(t * 3) * 0.03 * k;
      b.rotation.z += Math.sin(t * 38) * 0.04 * k * f;
      b.rotation.y += (-a.facing - b.rotation.y) * k;
      b.rotation.x += (-0.45 - b.rotation.x) * k;
      L.rotation.x *= 1 - k;
      R.rotation.x *= 1 - k;
      L.rotation.z += (-2.5 - L.rotation.z) * k;
      R.rotation.z += (2.5 - R.rotation.z) * k;
      const wide = 1 + 0.35 * k;
      const open = ey.scale.y + (1 - ey.scale.y) * k;
      ey.scale.set(wide, open * wide, wide);
    } else {
      ey.scale.x = 1;
      ey.scale.z = 1;
    }
    // The can kept level whatever its arm is doing, then tipped to pour.
    if (watering) {
      can.current!.rotation.set(
        -(R.rotation.x + b.rotation.x) + 0.55 + Math.sin(t * 3) * 0.08,
        0,
        -R.rotation.z,
      );
    }
    // The bottle kept upright whatever its arm is doing, and tipped out over
    // the crowd while it sprays.
    if (champagne) {
      bottle.current!.rotation.set(
        -(R.rotation.x + fig.rotation.x),
        0,
        -R.rotation.z - (popping ? 0.6 : 0),
      );
    }
  });

  const hit = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation();
    handlers.onSelect(resident.id);
  };

  return (
    <group
      ref={root}
      onClick={hit}
      onContextMenu={(e) => {
        e.stopPropagation();
        e.nativeEvent.preventDefault();
        handlers.onMenu(
          resident.id,
          e.nativeEvent.clientX,
          e.nativeEvent.clientY,
        );
      }}
      onPointerOver={(e) => {
        e.stopPropagation();
        handlers.onHover(resident.id);
      }}
      onPointerOut={() => handlers.onHover(null)}
    >
      {/* An easier target than the figure's own small parts. */}
      <mesh position-y={0.55}>
        <cylinderGeometry args={[0.45, 0.45, 1.2, 8]} />
        <meshBasicMaterial transparent opacity={0} depthWrite={false} />
      </mesh>
      <mesh ref={shadow} rotation-x={-Math.PI / 2} position-y={0.01}>
        <circleGeometry args={[0.36, 18]} />
        <meshBasicMaterial
          color="#000000"
          transparent
          opacity={0.18}
          depthWrite={false}
        />
      </mesh>
      {/* Water falling from the can onto the plot in front. */}
      <group ref={drops} position={[0.26, 0, 0.36]} visible={false}>
        {Array.from({ length: 6 }, (_, i) => (
          <mesh key={i}>
            <sphereGeometry args={[0.03, 5, 4]} />
            <meshBasicMaterial color="#9fd3f5" />
          </mesh>
        ))}
      </group>
      <group ref={boat} visible={false}>
        <Boat />
      </group>
      <group
        ref={floatRing}
        position={[0, 0.1, -0.35]}
        scale={1.6}
        visible={false}
      >
        <FloatRing index={hashString(resident.id)} />
      </group>
      <mesh ref={ring} rotation-x={-Math.PI / 2} position-y={0.02}>
        <torusGeometry args={[0.55, 0.035, 6, 36]} />
        <meshBasicMaterial color="#ffffff" />
      </mesh>
      <mesh
        ref={kinRing}
        rotation-x={-Math.PI / 2}
        position-y={0.025}
        visible={false}
      >
        <torusGeometry args={[0.66, 0.05, 6, 36]} />
        <meshBasicMaterial color={design.accent} />
      </mesh>
      <group ref={sparkles} visible={false}>
        {Array.from({ length: 5 }, (_, i) => (
          <mesh
            key={i}
            position={[
              Math.cos((i / 5) * Math.PI * 2) * 0.55,
              Math.sin(i * 2.1) * 0.25,
              Math.sin((i / 5) * Math.PI * 2) * 0.55,
            ]}
          >
            <octahedronGeometry args={[0.045]} />
            <meshBasicMaterial color="#fff3c4" />
          </mesh>
        ))}
      </group>

      <group ref={egg}>
        <mesh position-y={0.42} scale={[1, 1.3, 1]}>
          <sphereGeometry args={[0.32, 12, 10]} />
          <Mat color={mood === "failed" ? "#9e9e9e" : "#fff4dc"} />
        </mesh>
        <mesh position-y={0.42} rotation-x={Math.PI / 2}>
          <torusGeometry args={[0.315, 0.04, 6, 18]} />
          <Mat color={mood === "failed" ? "#6d6d6d" : design.coat} />
        </mesh>
      </group>

      <group ref={figure}>
        <group ref={body}>
          <design.Torso />
          <group
            position-y={RIG.head}
            scale={resident.depth > 0 ? YOUNG_HEAD : 1}
          >
            <design.Head eyes={eyes} />
          </group>
          {design.Tail && (
            <group ref={tail} position={[0, RIG.tail.y, RIG.tail.z]}>
              <design.Tail />
            </group>
          )}
          <group ref={armL} position={[-RIG.shoulder.x, RIG.shoulder.y, 0]}>
            <design.Arm side={-1} />
          </group>
          <group ref={armR} position={[RIG.shoulder.x, RIG.shoulder.y, 0]}>
            <design.Arm side={1} />
            <group
              ref={bottle}
              position={[0, -RIG.hand, 0.03]}
              scale={1.5}
              visible={false}
            >
              <Champagne cork={cork} spray={spray} />
            </group>
            <group
              ref={drink}
              position={[0, -RIG.hand - 0.02, 0.03]}
              scale={1.4}
            >
              <Drink juice={design.accent} />
            </group>
            <group ref={can} position-y={-RIG.hand} scale={1.7} visible={false}>
              <WateringCan />
            </group>
            <group ref={tool} position-y={-RIG.hand}>
              <Tool work={work} />
            </group>
          </group>
          <group
            ref={book}
            position={[0, RIG.shoulder.y - 0.22, RIG.front + 0.2]}
            rotation-x={-0.5}
          >
            {[-1, 1].map((s) => (
              <mesh key={s} position-x={s * 0.1} rotation-y={s * -0.3}>
                <boxGeometry args={[0.2, 0.26, 0.02]} />
                <Mat color={s < 0 ? design.accent : "#fdf6e3"} />
              </mesh>
            ))}
          </group>
          <group
            ref={crate}
            position={[0, RIG.shoulder.y - 0.18, RIG.front + 0.2]}
          >
            <mesh>
              <boxGeometry args={[0.32, 0.28, 0.28]} />
              <Mat color="#c08a5b" />
            </mesh>
            {[-0.1, 0.1].map((y) => (
              <mesh key={y} position-y={y}>
                <boxGeometry args={[0.34, 0.04, 0.3]} />
                <Mat color="#9a6b47" />
              </mesh>
            ))}
            <group ref={produce} position-y={0.16}>
              {[-0.08, 0, 0.08].map((x, k) => (
                <mesh key={x} position={[x, 0, k === 1 ? 0.04 : -0.03]}>
                  <sphereGeometry args={[0.06, 6, 5]} />
                </mesh>
              ))}
            </group>
          </group>
          <group
            ref={parcel}
            position={[0, RIG.shoulder.y - 0.18, RIG.front + 0.2]}
          >
            <mesh>
              <boxGeometry args={[0.34, 0.26, 0.26]} />
              <Mat color="#c9925b" />
            </mesh>
            <mesh>
              <boxGeometry args={[0.36, 0.05, 0.28]} />
              <Mat color={design.accent} />
            </mesh>
          </group>
        </group>
        <group ref={footL}>
          <design.Foot side={-1} />
        </group>
        <group ref={footR}>
          <design.Foot side={1} />
        </group>
        <group ref={station} position-z={0.5}>
          <Station work={work} accent={design.accent} />
        </group>
        <group ref={pedestal} position={[0.62, 0, 0.15]}>
          <mesh position-y={0.17}>
            <cylinderGeometry args={[0.16, 0.19, 0.34, 8]} />
            <Mat color="#e8e2d4" />
          </mesh>
          <group position-y={0.47} rotation-y={0.4}>
            <mesh>
              <boxGeometry args={[0.3, 0.24, 0.24]} />
              <Mat color="#c9925b" />
            </mesh>
            <mesh>
              <boxGeometry args={[0.32, 0.05, 0.26]} />
              <Mat color={design.accent} />
            </mesh>
          </group>
        </group>
      </group>
    </group>
  );
}

function poseWork(
  work: Work,
  t: number,
  local: number,
  b: THREE.Group,
  L: THREE.Group,
  R: THREE.Group,
) {
  switch (work) {
    case "typing":
      L.rotation.x = -1.25 + Math.sin(t * 22) * 0.12;
      R.rotation.x = -1.25 + Math.sin(t * 22 + Math.PI) * 0.12;
      b.rotation.x = 0.06;
      break;
    case "hammering": {
      const s = (local * 2.2) % 1;
      R.rotation.x =
        s < 0.7 ? -0.9 - (s / 0.7) * 1.7 : -2.6 + ((s - 0.7) / 0.3) * 1.7;
      L.rotation.x = -0.5;
      b.rotation.x = s > 0.7 ? 0.08 : 0;
      break;
    }
    case "reading":
      L.rotation.x = -1.05;
      R.rotation.x = -1.05;
      L.rotation.z = 0.2;
      R.rotation.z = -0.2;
      b.rotation.z = Math.sin(t * 0.8) * 0.05;
      break;
    case "stirring":
      R.rotation.x = -1.0 + Math.sin(t * 4) * 0.2;
      R.rotation.z = 0.12 + Math.cos(t * 4) * 0.25;
      b.rotation.z = Math.sin(t * 4) * 0.04;
      break;
    case "sweeping":
      L.rotation.x = -0.75;
      R.rotation.x = -0.75;
      b.rotation.y = Math.sin(t * 3) * 0.4;
      break;
  }
}

/** A watering can, its spout to the front. */
function WateringCan() {
  return (
    <group position-y={-0.06}>
      <mesh>
        <cylinderGeometry args={[0.07, 0.075, 0.12, 8]} />
        <Mat color="#4f95c4" />
      </mesh>
      <mesh position={[0, 0.03, 0.1]} rotation-x={1.05}>
        <cylinderGeometry args={[0.012, 0.016, 0.15, 5]} />
        <Mat color="#4f95c4" />
      </mesh>
      <mesh position={[0, 0.09, -0.02]} rotation-y={Math.PI / 2}>
        <torusGeometry args={[0.045, 0.012, 4, 8, Math.PI]} />
        <Mat color="#3b78a3" />
      </mesh>
    </group>
  );
}
