import { createContext, useContext, useEffect, useState } from "react";

/**
 * Where the residents spend the day. Everything happens as it always does;
 * only what it's drawn as changes.
 */
export type Environment = "meadow" | "moon" | "athens" | "office";

export const ENVIRONMENTS: readonly Environment[] = [
  "meadow",
  "moon",
  "athens",
  "office",
];

/** Whether the weather where Ryan is reaches it. */
export const OUTDOORS: Record<Environment, boolean> = {
  meadow: true,
  moon: false,
  athens: true,
  office: false,
};

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** One turn of days: every environment once, in an order of its own. */
function turn(index: number): Environment[] {
  const rng = mulberry32(index * 2654435761 + 17);
  const order = [...ENVIRONMENTS];
  for (let i = order.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [order[i], order[j]] = [order[j], order[i]];
  }
  return order;
}

/**
 * The environment on a day where Ryan is. Each run of four days has every
 * environment once, and the first of a run is never the last of the one
 * before: swapping the first two leaves a run's last day as it was.
 */
export function environmentOn(date: Date): Environment {
  const day = Math.floor(
    Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000,
  );
  const n = ENVIRONMENTS.length;
  const index = Math.floor(day / n);
  const order = turn(index);
  if (order[0] === turn(index - 1)[n - 1]) {
    [order[0], order[1]] = [order[1], order[0]];
  }
  return order[day - index * n];
}

/** Sent by the View menu's "Next environment", to look round them all. */
const NEXT_EVENT = "tethys:next-environment";

/**
 * Today's environment, moving on at midnight. `fixed` holds it still. The
 * View menu steps on from it, until a reload or midnight.
 */
export function useToday(fixed?: Environment): Environment {
  const [today, setToday] = useState(() => environmentOn(new Date()));
  const [skip, setSkip] = useState(0);
  useEffect(() => {
    const next = () => setSkip((n) => n + 1);
    window.addEventListener(NEXT_EVENT, next);
    return () => window.removeEventListener(NEXT_EVENT, next);
  }, []);
  useEffect(() => {
    if (fixed) return;
    let timer = 0;
    const wake = () => {
      const now = new Date();
      setToday(environmentOn(now));
      setSkip(0);
      const midnight = new Date(now);
      midnight.setHours(24, 0, 1, 0);
      timer = window.setTimeout(wake, midnight.getTime() - now.getTime());
    };
    wake();
    return () => clearTimeout(timer);
  }, [fixed]);
  const from = ENVIRONMENTS.indexOf(fixed ?? today);
  return ENVIRONMENTS[(from + skip) % ENVIRONMENTS.length];
}

export const EnvironmentContext = createContext<Environment>("meadow");

export const useEnvironment = () => useContext(EnvironmentContext);
