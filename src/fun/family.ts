import { ANIMAL_NAMES, type AnimalName } from "./animals";

/** Where each workspace's animal is kept, so it stays the same animal. */
const STORAGE_KEY = "tethys.fun.animals";

interface Member {
  id: string;
  parent: string | null;
}

/**
 * Each workspace's animal. A child is its parent's animal; anyone else keeps
 * the one it has, or gets one at random that nobody alive is. Past that many
 * families, one of the least used. A child keeps its animal once its parent is
 * gone. Only the live are kept, so a deleted workspace's animal is free again.
 */
export function allocate(
  held: ReadonlyMap<string, AnimalName>,
  live: Member[],
  random: () => number = Math.random,
): Map<string, AnimalName> {
  const byId = new Map(live.map((m) => [m.id, m]));
  const depth = (m: Member) => {
    let d = 0;
    for (let p = m.parent; p && byId.has(p) && d < 8; d++) {
      p = byId.get(p)!.parent;
    }
    return d;
  };
  const next = new Map<string, AnimalName>();
  for (const m of live) {
    const animal = held.get(m.id);
    if (animal && ANIMAL_NAMES.includes(animal)) next.set(m.id, animal);
  }
  // Parents first, so a new child finds its parent's animal already set.
  for (const m of [...live].sort((a, b) => depth(a) - depth(b))) {
    if (next.has(m.id)) continue;
    const inherited = m.parent ? next.get(m.parent) : undefined;
    next.set(m.id, inherited ?? pickFree(next, random));
  }
  return next;
}

function pickFree(
  taken: ReadonlyMap<string, AnimalName>,
  random: () => number,
): AnimalName {
  const uses = new Map(ANIMAL_NAMES.map((a) => [a, 0]));
  for (const a of taken.values()) uses.set(a, uses.get(a)! + 1);
  const least = Math.min(...uses.values());
  const choices = ANIMAL_NAMES.filter((a) => uses.get(a) === least);
  return choices[Math.floor(random() * choices.length)];
}

export function loadAnimals(): Map<string, AnimalName> {
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    return new Map(Object.entries(raw)) as Map<string, AnimalName>;
  } catch {
    return new Map();
  }
}

export function saveAnimals(animals: ReadonlyMap<string, AnimalName>) {
  localStorage.setItem(
    STORAGE_KEY,
    JSON.stringify(Object.fromEntries(animals)),
  );
}

/** Whether two allocations give everyone the same animal. */
export function sameAnimals(
  a: ReadonlyMap<string, AnimalName>,
  b: ReadonlyMap<string, AnimalName>,
): boolean {
  return a.size === b.size && [...a].every(([id, x]) => b.get(id) === x);
}
