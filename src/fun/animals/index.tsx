import { fox } from "./fox";
import { rabbit } from "./rabbit";
import { bear } from "./bear";
import { panda } from "./panda";
import { cat } from "./cat";
import { dog } from "./dog";
import { pig } from "./pig";
import { frog } from "./frog";
import { penguin } from "./penguin";
import { owl } from "./owl";
import { mouse } from "./mouse";
import { raccoon } from "./raccoon";
import { sheep } from "./sheep";
import { cow } from "./cow";
import { koala } from "./koala";
import { tiger } from "./tiger";
import { elephant } from "./elephant";
import { deer } from "./deer";
import { hedgehog } from "./hedgehog";
import { otter } from "./otter";
import type { AnimalDesign } from "./rig";

/** Every animal a family can be, one file each. */
export const ANIMALS = {
  fox,
  rabbit,
  bear,
  panda,
  cat,
  dog,
  pig,
  frog,
  penguin,
  owl,
  mouse,
  raccoon,
  sheep,
  cow,
  koala,
  tiger,
  elephant,
  deer,
  hedgehog,
  otter,
} satisfies Record<string, AnimalDesign>;

export type AnimalName = keyof typeof ANIMALS;
export const ANIMAL_NAMES = Object.keys(ANIMALS) as AnimalName[];
