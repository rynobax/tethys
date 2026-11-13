import { useMemo, useRef, useState, type RefObject } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { STORE_MAX, type Garden, type Sim, type Vec2 } from "./sim";

/** Each crop's colour once picked: tomato, pumpkin, carrot, cabbage, sunflower. */
export const CROP_COLORS = [
  "#e0483a",
  "#e8892b",
  "#f2a03a",
  "#8cc46a",
  "#f5c518",
];

const LEAF = "#4f9a3c";
const SOIL = "#6b4a32";
const WOOD = "#9a6b47";

function Mat({ color }: { color: string }) {
  return <meshStandardMaterial color={color} flatShading roughness={0.8} />;
}

const smooth = (from: number, to: number, x: number) => {
  const t = Math.min(1, Math.max(0, (x - from) / (to - from)));
  return t * t * (3 - 2 * t);
};

/** The leaves, which grow from a sprout, and the crop, which swells late. */
function CropModel({
  crop,
  leaves,
  fruit,
}: {
  crop: number;
  leaves: RefObject<THREE.Group | null>;
  fruit: RefObject<THREE.Group | null>;
}) {
  const color = CROP_COLORS[crop];
  switch (crop) {
    case 0: // Tomato: a bush on a stake, hung with red fruit.
      return (
        <>
          <group ref={leaves}>
            <mesh position-y={0.24}>
              <cylinderGeometry args={[0.012, 0.012, 0.48, 4]} />
              <Mat color={WOOD} />
            </mesh>
            <mesh position-y={0.2} scale={[1, 1.2, 1]}>
              <icosahedronGeometry args={[0.14, 0]} />
              <Mat color={LEAF} />
            </mesh>
          </group>
          <group ref={fruit}>
            {[
              [0.1, 0.16, 0.07],
              [-0.09, 0.24, 0.08],
              [0.02, 0.3, 0.1],
              [-0.05, 0.12, 0.1],
            ].map(([x, y, z], i) => (
              <mesh key={i} position={[x, y, z]}>
                <sphereGeometry args={[0.05, 7, 5]} />
                <Mat color={color} />
              </mesh>
            ))}
          </group>
        </>
      );
    case 1: // Pumpkin: broad leaves, and a fat pumpkin sat on the soil.
      return (
        <>
          <group ref={leaves}>
            {[0, 2.1, 4.2].map((a) => (
              <mesh
                key={a}
                position={[Math.sin(a) * 0.13, 0.05, Math.cos(a) * 0.1 - 0.04]}
                scale={[1, 0.35, 1]}
              >
                <sphereGeometry args={[0.1, 6, 4]} />
                <Mat color={LEAF} />
              </mesh>
            ))}
          </group>
          <group ref={fruit}>
            <mesh position={[0, 0.1, 0.04]} scale={[1.15, 0.8, 1]}>
              <sphereGeometry args={[0.13, 10, 7]} />
              <Mat color={color} />
            </mesh>
            <mesh position={[0, 0.21, 0.04]}>
              <cylinderGeometry args={[0.012, 0.018, 0.06, 4]} />
              <Mat color="#5b7a2e" />
            </mesh>
          </group>
        </>
      );
    case 2: // Carrot: feathery tops, the carrot's shoulder showing at the soil.
      return (
        <>
          <group ref={leaves}>
            {[-0.4, 0, 0.4].map((tilt) => (
              <mesh key={tilt} position-y={0.13} rotation-z={tilt}>
                <coneGeometry args={[0.035, 0.24, 4]} />
                <Mat color={LEAF} />
              </mesh>
            ))}
          </group>
          <group ref={fruit}>
            <mesh position-y={0.02} rotation-x={Math.PI}>
              <coneGeometry args={[0.06, 0.12, 6]} />
              <Mat color={color} />
            </mesh>
          </group>
        </>
      );
    case 3: // Cabbage: loose outer leaves round a tight head.
      return (
        <>
          <group ref={leaves}>
            <mesh position-y={0.06} scale={[1, 0.45, 1]}>
              <icosahedronGeometry args={[0.16, 0]} />
              <Mat color={LEAF} />
            </mesh>
          </group>
          <group ref={fruit}>
            <mesh position-y={0.12}>
              <icosahedronGeometry args={[0.1, 1]} />
              <Mat color={color} />
            </mesh>
          </group>
        </>
      );
    default: // Sunflower: a tall stalk, and a flower turned to the viewer.
      return (
        <>
          <group ref={leaves}>
            <mesh position-y={0.25}>
              <cylinderGeometry args={[0.018, 0.022, 0.5, 5]} />
              <Mat color={LEAF} />
            </mesh>
            {[-1, 1].map((s) => (
              <mesh
                key={s}
                position={[s * 0.07, 0.2, 0]}
                rotation-z={s * -0.6}
                scale={[1, 0.3, 0.6]}
              >
                <sphereGeometry args={[0.08, 5, 4]} />
                <Mat color={LEAF} />
              </mesh>
            ))}
          </group>
          <group ref={fruit} position={[0, 0.52, 0.02]} rotation-x={0.35}>
            <mesh rotation-x={Math.PI / 2}>
              <cylinderGeometry args={[0.13, 0.13, 0.025, 10]} />
              <Mat color={color} />
            </mesh>
            <mesh position-z={0.014} rotation-x={Math.PI / 2}>
              <cylinderGeometry args={[0.06, 0.06, 0.02, 8]} />
              <Mat color="#6b4423" />
            </mesh>
          </group>
        </>
      );
  }
}

/** One plot: bare, a sprout, grown, or ripe and nodding in the breeze. */
function Plant({ at, index, sim }: { at: Vec2; index: number; sim: Sim }) {
  const group = useRef<THREE.Group>(null);
  const leaves = useRef<THREE.Group>(null);
  const fruit = useRef<THREE.Group>(null);
  const [crop, setCrop] = useState(() => sim.plot(index)?.crop ?? 0);
  useFrame(() => {
    const p = sim.plot(index);
    const g = group.current;
    if (!g) return;
    g.visible = !!p && p.growth >= 0;
    if (!p || p.growth < 0) return;
    if (p.crop !== crop) setCrop(p.crop);
    leaves.current?.scale.setScalar(0.15 + 0.85 * smooth(0, 0.7, p.growth));
    fruit.current?.scale.setScalar(Math.max(0.001, smooth(0.5, 1, p.growth)));
    g.rotation.z =
      p.growth >= 1 ? Math.sin(sim.time() * 2 + index * 1.7) * 0.06 : 0;
  });
  return (
    <group ref={group} position={[at.x, 0.05, at.z]} scale={1.6}>
      <CropModel crop={crop} leaves={leaves} fruit={fruit} />
    </group>
  );
}

/** A crate of produce, whose colour says what's in it. */
export function ProduceCrate({ material }: { material: THREE.Material }) {
  return (
    <>
      <mesh>
        <boxGeometry args={[0.26, 0.18, 0.2]} />
        <Mat color="#c08a5b" />
      </mesh>
      {[-0.07, 0, 0.07].map((x, i) => (
        <mesh
          key={x}
          position={[x, 0.1, i === 1 ? 0.03 : -0.02]}
          material={material}
        >
          <sphereGeometry args={[0.055, 6, 5]} />
        </mesh>
      ))}
    </>
  );
}

/** A crop material per kind, shared by everything showing that crop. */
export function useCropMaterials() {
  return useMemo(
    () =>
      CROP_COLORS.map(
        (color) =>
          new THREE.MeshStandardMaterial({
            color,
            flatShading: true,
            roughness: 0.8,
          }),
      ),
    [],
  );
}

/** Stacked three rows high, widest at the bottom. */
const STACK: [number, number, number][] = [
  [-0.28, 0, 0],
  [0, 0, 0],
  [0.28, 0, 0],
  [-0.14, 0, -0.22],
  [0.14, 0, -0.22],
  [-0.14, 0.2, -0.05],
  [0.14, 0.2, -0.05],
  [0, 0.2, -0.27],
  [0, 0.4, -0.15],
];

/** The harvest at the shed: one crate per crop brought in. */
function Store({ at, sim }: { at: Vec2; sim: Sim }) {
  const materials = useCropMaterials();
  // Each crate's crop, so a crate is only rebuilt when that changes.
  const [crops, setCrops] = useState<number[]>([]);
  useFrame(() => {
    const stored = sim.storedCrops();
    if (
      stored.length !== crops.length ||
      stored.some((c, i) => c !== crops[i])
    ) {
      setCrops([...stored]);
    }
  });
  return (
    <group position={[at.x, 0.09, at.z]}>
      {crops.slice(0, STORE_MAX).map((crop, i) => (
        <group
          key={i}
          position={STACK[i]}
          rotation-y={Math.sin(i * 7.3) * 0.15}
        >
          <ProduceCrate material={materials[crop]} />
        </group>
      ))}
    </group>
  );
}

function Shed({ at }: { at: Vec2 }) {
  return (
    <group position={[at.x, 0, at.z]}>
      <mesh position-y={0.32}>
        <boxGeometry args={[0.8, 0.64, 0.6]} />
        <Mat color="#b0784a" />
      </mesh>
      <mesh position-y={0.82} rotation-y={Math.PI / 4} scale={[1, 1, 0.78]}>
        <coneGeometry args={[0.66, 0.36, 4]} />
        <Mat color="#a8483a" />
      </mesh>
      <mesh position={[0, 0.24, 0.301]}>
        <planeGeometry args={[0.28, 0.46]} />
        <meshStandardMaterial color="#6e4528" />
      </mesh>
    </group>
  );
}

/** Rows of plots, each row a raised bed of soil, and the shed at the end. */
export function GardenArea({ garden, sim }: { garden: Garden; sim: Sim }) {
  const beds = useMemo(() => {
    const rows = new Map<number, Vec2[]>();
    for (const p of garden.plots) {
      rows.set(p.z, [...(rows.get(p.z) ?? []), p]);
    }
    return [...rows].map(([z, ps]) => ({
      z,
      xMin: Math.min(...ps.map((p) => p.x)) - 0.38,
      xMax: Math.max(...ps.map((p) => p.x)) + 0.38,
    }));
  }, [garden]);
  return (
    <>
      {beds.map((b) => (
        <mesh key={b.z} position={[(b.xMin + b.xMax) / 2, 0.025, b.z]}>
          <boxGeometry args={[b.xMax - b.xMin, 0.05, 0.5]} />
          <Mat color={SOIL} />
        </mesh>
      ))}
      {garden.plots.map((p, i) => (
        <Plant key={i} at={p} index={i} sim={sim} />
      ))}
      <Shed at={garden.shed} />
      <Store at={garden.store} sim={sim} />
    </>
  );
}
