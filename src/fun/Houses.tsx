import { useEffect, useMemo, useState } from "react";
import { useFrame, type ThreeEvent } from "@react-three/fiber";
import * as THREE from "three";
import type { Folder } from "../types";
import { Mat } from "./animals/rig";
import type { House, Sim } from "./sim";
import { hashString } from "./spec";

const WALLS = [
  "#f4d6a0",
  "#c9e4f2",
  "#f6c6c6",
  "#d4ecc4",
  "#e3d4f2",
  "#fbe3b8",
];
const ROOFS = [
  "#c0533f",
  "#4b6fa8",
  "#a0506e",
  "#3f8a5a",
  "#7a4f9a",
  "#b8743a",
];
const DOOR = "#6e4528";
const GLASS_DARK = "#5a6f8a";
const GLASS_LIT = "#ffd36b";

const WIDTH = 2.1;
const HEIGHT = 1.35;
const DEPTH = 0.3;
const SIGN = { w: 1.9, h: 0.38, y: 1.08 };
const DOOR_W = 0.42;
const DOOR_H = 0.72;

/** Text on a canvas, shrunk to fit and cut short past that, for a texture. */
function textTexture(
  text: string,
  width: number,
  height: number,
  draw: (ctx: CanvasRenderingContext2D) => void,
) {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d")!;
  draw(ctx);
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  let size = height * 0.62;
  const font = () => `700 ${size}px -apple-system, system-ui, sans-serif`;
  ctx.font = font();
  while (size > height * 0.34 && ctx.measureText(text).width > width * 0.9) {
    size -= 2;
    ctx.font = font();
  }
  let shown = text;
  while (shown.length > 1 && ctx.measureText(shown).width > width * 0.9) {
    shown = shown.slice(0, -2) + "…";
  }
  ctx.fillStyle = "#3b2a1c";
  ctx.fillText(shown, width / 2, height / 2 + size * 0.04);
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = 4;
  return texture;
}

/** The folder's name painted on a board over the door. */
function Sign({ name }: { name: string }) {
  const texture = useMemo(
    () =>
      textTexture(name, 600, 120, (ctx) => {
        ctx.fillStyle = "#fbf3e2";
        ctx.fillRect(0, 0, 600, 120);
      }),
    [name],
  );
  useEffect(() => () => texture.dispose(), [texture]);
  return (
    <group position={[0, SIGN.y, 0.01]}>
      <mesh>
        <boxGeometry args={[SIGN.w + 0.06, SIGN.h + 0.06, 0.04]} />
        <Mat color={DOOR} />
      </mesh>
      <mesh position-z={0.021}>
        <planeGeometry args={[SIGN.w, SIGN.h]} />
        <meshBasicMaterial map={texture} toneMapped={false} />
      </mesh>
    </group>
  );
}

/**
 * A folder's house front: its name over the door, the door shut and the
 * windows lit while its folk are home, and open while they're out. Clicked,
 * it opens or shuts the folder.
 */
function HouseFront({
  house,
  folder,
  index,
  sim,
  onToggle,
}: {
  house: House;
  folder: Folder;
  index: number;
  sim: Sim;
  onToggle: (folder: Folder) => void;
}) {
  // Who's in, read off the sim: the lights go on.
  const [anyone, setAnyone] = useState(false);
  useFrame(() => {
    const now = sim.indoors(index).length > 0;
    if (now !== anyone) setAnyone(now);
  });
  // Its own colours, kept whatever order the folders are in.
  const hue = hashString(folder.id);
  const wall = WALLS[hue % WALLS.length];
  const roof = ROOFS[(hue >>> 3) % ROOFS.length];
  const glass = anyone ? GLASS_LIT : GLASS_DARK;
  const open = !folder.collapsed;
  const roofShape = useMemo(
    () =>
      new THREE.CylinderGeometry(1, 1, DEPTH + 0.1, 3).rotateX(-Math.PI / 2),
    [],
  );
  const click = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation();
    onToggle(folder);
  };
  return (
    <group
      position={[house.front.x, 0, house.front.z]}
      onClick={click}
      onPointerOver={(e) => {
        e.stopPropagation();
        document.body.style.cursor = "pointer";
      }}
      onPointerOut={() => {
        document.body.style.cursor = "";
      }}
    >
      <mesh position={[0, HEIGHT / 2, -DEPTH / 2]}>
        <boxGeometry args={[WIDTH, HEIGHT, DEPTH]} />
        <Mat color={wall} />
      </mesh>
      <mesh
        geometry={roofShape}
        position={[0, HEIGHT + 0.27, -DEPTH / 2]}
        scale={[1.35, 0.55, 1]}
      >
        <Mat color={roof} />
      </mesh>
      {/* The doorway, and the door on its hinge, swung in while they're out. */}
      <mesh position={[0, DOOR_H / 2, 0.004]}>
        <planeGeometry args={[DOOR_W, DOOR_H]} />
        <meshBasicMaterial color="#2a1d14" />
      </mesh>
      <group position={[-DOOR_W / 2, 0, 0.02]} rotation-y={open ? -1.3 : 0}>
        <mesh position={[DOOR_W / 2, DOOR_H / 2, 0]}>
          <boxGeometry args={[DOOR_W, DOOR_H, 0.04]} />
          <Mat color={DOOR} />
        </mesh>
        <mesh position={[DOOR_W - 0.07, DOOR_H / 2, 0.03]}>
          <sphereGeometry args={[0.03, 6, 4]} />
          <Mat color="#e8c24a" />
        </mesh>
      </group>
      <mesh position={[0, 0.025, 0.16]}>
        <boxGeometry args={[0.62, 0.05, 0.26]} />
        <Mat color="#bdb8b0" />
      </mesh>
      {[-1, 1].map((s) => (
        <group key={s} position={[s * 0.68, 0.5, 0.01]}>
          <mesh>
            <boxGeometry args={[0.42, 0.38, 0.03]} />
            <Mat color="#ffffff" />
          </mesh>
          <mesh position-z={0.017}>
            <planeGeometry args={[0.34, 0.3]} />
            <meshBasicMaterial color={glass} toneMapped={false} />
          </mesh>
        </group>
      ))}
      <Sign name={folder.name} />
    </group>
  );
}

/** A house for each folder, as many as the street has room for. */
export function Houses({
  houses,
  folders,
  sim,
  onToggle,
}: {
  houses: House[];
  folders: Folder[];
  sim: Sim;
  onToggle: (folder: Folder) => void;
}) {
  return (
    <>
      {houses.slice(0, folders.length).map((h, i) => (
        <HouseFront
          key={folders[i].id}
          house={h}
          folder={folders[i]}
          index={i}
          sim={sim}
          onToggle={onToggle}
        />
      ))}
    </>
  );
}
