import * as THREE from "three";
import { Blush, DotEyes, Mat, RIG, SimpleArm, type AnimalDesign } from "./rig";

const COAT = "#6e4f3a";
const SPINE = "#5a3d2b";
const FACE = "#ecd6b4";
const PAW = "#c9a882";
const NOSE = "#1d1d1f";

/**
 * A ball of spines in one mesh: a pyramid raised on each face of an
 * icosahedron that `keep` accepts (given the face's outward direction), all
 * leaning back by `sweep`, as quills lie.
 */
function spines(
  radius: number,
  length: number,
  keep: (n: THREE.Vector3) => boolean,
  sweep = 0.6,
): THREE.BufferGeometry {
  const base = new THREE.IcosahedronGeometry(radius, 1);
  const p = base.getAttribute("position");
  const out: number[] = [];
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const n = new THREE.Vector3();
  const tip = new THREE.Vector3();
  for (let i = 0; i < p.count; i += 3) {
    a.fromBufferAttribute(p, i);
    b.fromBufferAttribute(p, i + 1);
    c.fromBufferAttribute(p, i + 2);
    n.copy(a).add(b).add(c).divideScalar(3);
    const centre = n.clone();
    n.normalize();
    if (!keep(n)) continue;
    tip
      .copy(n)
      .add(new THREE.Vector3(0, 0, -sweep))
      .normalize()
      .multiplyScalar(length)
      .add(centre);
    for (const [u, v] of [
      [a, b],
      [b, c],
      [c, a],
    ]) {
      out.push(u.x, u.y, u.z, v.x, v.y, v.z, tip.x, tip.y, tip.z);
    }
  }
  base.dispose();
  const geo = new THREE.BufferGeometry();
  geo.setAttribute("position", new THREE.Float32BufferAttribute(out, 3));
  geo.computeVertexNormals();
  return geo;
}

/** The crown: over the top and back of the head, clear of the face. */
const CROWN = spines(
  0.25,
  0.11,
  (n) => n.z < 0.15 || (n.y > 0.55 && n.z < 0.75),
);
/** The back, from the shoulders down, leaving the belly bare. */
const BACK = spines(0.24, 0.12, (n) => n.z < -0.1 && n.y > -0.75, 0.3);

export const hedgehog: AnimalDesign = {
  coat: COAT,
  accent: "#e63946",
  top: 1.25,
  Head: ({ eyes }) => (
    <group>
      <mesh scale={[1.05, 0.95, 1]}>
        <sphereGeometry args={[0.24, 12, 10]} />
        <Mat color={FACE} />
      </mesh>
      <mesh geometry={CROWN} position={[0, 0.03, -0.03]} scale={[1.08, 1, 1]}>
        <Mat color={SPINE} />
      </mesh>
      {/* The pointed snout, its dark button nose up at the tip. */}
      <mesh position={[0, -0.06, 0.25]} rotation-x={Math.PI / 2 - 0.25}>
        <coneGeometry args={[0.09, 0.18, 8]} />
        <Mat color={FACE} />
      </mesh>
      <mesh position={[0, -0.035, 0.335]}>
        <sphereGeometry args={[0.036, 6, 5]} />
        <Mat color={NOSE} />
      </mesh>
      {/* Small round ears peeking out of the spines. */}
      {[-1, 1].map((s) => (
        <mesh
          key={s}
          position={[s * 0.17, 0.13, 0.1]}
          rotation-z={s * -0.5}
          scale={[1, 1, 0.5]}
        >
          <sphereGeometry args={[0.055, 7, 5]} />
          <Mat color={PAW} />
        </mesh>
      ))}
      <DotEyes eyes={eyes} y={0.02} x={0.095} z={0.205} />
      <Blush y={-0.06} x={0.15} z={0.18} />
    </group>
  ),
  Torso: () => (
    <group position-y={(RIG.hip + RIG.chest) / 2}>
      <mesh scale={[1, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={COAT} />
      </mesh>
      <mesh position={[0, -0.02, 0.12]} scale={[0.8, 1.05, 0.55]}>
        <sphereGeometry args={[0.2, 14, 12]} />
        <Mat color={FACE} />
      </mesh>
      <mesh geometry={BACK} position={[0, 0.02, -0.02]} scale={[1, 1.15, 0.95]}>
        <Mat color={SPINE} />
      </mesh>
    </group>
  ),
  Arm: () => <SimpleArm color={COAT} hand={PAW} />,
  Foot: () => (
    <mesh scale={[1, 0.7, 1.2]}>
      <sphereGeometry args={[0.08, 8, 6]} />
      <Mat color={PAW} />
    </mesh>
  ),
};
