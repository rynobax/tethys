import { fox } from "./fox";
import {
  Blush,
  DotEyes,
  Mat,
  RIG,
  SimpleArm,
  SimpleFoot,
  type AnimalDesign,
} from "./rig";

/**
 * Round ears and a plain face in an animal's colours, standing in for its
 * design until it has one.
 */
function placeholder(coat: string, accent: string): AnimalDesign {
  return {
    coat,
    accent,
    top: 1.2,
    Head: ({ eyes }) => (
      <group>
        <mesh>
          <sphereGeometry args={[0.25, 12, 10]} />
          <Mat color={coat} />
        </mesh>
        {[-1, 1].map((s) => (
          <mesh key={s} position={[s * 0.17, 0.19, 0]} scale={[1, 1, 0.5]}>
            <sphereGeometry args={[0.08, 8, 6]} />
            <Mat color={coat} />
          </mesh>
        ))}
        <DotEyes eyes={eyes} y={0.03} x={0.1} z={0.22} />
        <Blush y={-0.05} x={0.16} z={0.19} />
      </group>
    ),
    Torso: () => (
      <mesh position-y={(RIG.hip + RIG.chest) / 2} scale={[1, 1.15, 0.95]}>
        <sphereGeometry args={[0.23, 12, 10]} />
        <Mat color={coat} />
      </mesh>
    ),
    Arm: () => <SimpleArm color={coat} />,
    Foot: () => <SimpleFoot color={coat} />,
  };
}

/** Every animal a family can be, each in its own file once designed. */
export const ANIMALS = {
  fox,
  rabbit: placeholder("#e9e4dc", "#ff7aa8"),
  bear: placeholder("#8a5a3b", "#3f8ae0"),
  panda: placeholder("#f4f4f2", "#4caf50"),
  cat: placeholder("#9aa3ad", "#ffb84d"),
  dog: placeholder("#d9a66b", "#e04f5f"),
  pig: placeholder("#f6b5c0", "#7a5cff"),
  frog: placeholder("#7cc35a", "#ff6f61"),
  penguin: placeholder("#2c3440", "#ffc93c"),
  owl: placeholder("#a07850", "#6fc2ff"),
  mouse: placeholder("#b8b2ab", "#ff8c42"),
  raccoon: placeholder("#8d8f94", "#9b5de5"),
  sheep: placeholder("#fbf7ee", "#4d96ff"),
  cow: placeholder("#ffffff", "#2ec4b6"),
  koala: placeholder("#a3aab3", "#f15bb5"),
  tiger: placeholder("#f29a38", "#00a6a6"),
  elephant: placeholder("#9fa8b8", "#ff9f1c"),
  deer: placeholder("#b47a4a", "#43aa8b"),
  hedgehog: placeholder("#7a5c45", "#e63946"),
  otter: placeholder("#7b5236", "#48cae4"),
} satisfies Record<string, AnimalDesign>;

export type AnimalName = keyof typeof ANIMALS;
export const ANIMAL_NAMES = Object.keys(ANIMALS) as AnimalName[];
