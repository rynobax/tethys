import { useEffect, useState } from "react";
import * as THREE from "three";
import type { Palette } from "./palette";

/**
 * The weather where Ryan is, as the meadows show it. Found by IP (geojs) and
 * looked up on Open-Meteo, neither of which wants a key.
 */
export interface Weather {
  /** How much of the sky is cloud, 0 to 1. */
  clouds: number;
  fog: boolean;
  /** What's falling, and how hard, 0 to 1. */
  fall: { kind: "rain" | "snow"; rate: number } | null;
  /** Thunder and lightning. */
  storm: boolean;
  /**
   * Where the wind blows to across the ground, in m/s, taking the top of the
   * screen as north: +x is east, +z south.
   */
  wind: { x: number; z: number };
}

export const CLEAR: Weather = {
  clouds: 0,
  fog: false,
  fall: null,
  storm: false,
  wind: { x: 0, z: 0 },
};

/** Open-Meteo's current conditions, as asked for in `lookup`. */
interface Report {
  /** WMO weather interpretation code. */
  code: number;
  /** Percent. */
  cloud: number;
  /** m/s. */
  speed: number;
  /** Degrees the wind comes from, 0 for north. */
  from: number;
}

function fromReport({ code, cloud, speed, from }: Report): Weather {
  const rain = (rate: number) => ({ kind: "rain" as const, rate });
  const snow = (rate: number) => ({ kind: "snow" as const, rate });
  const falls: Record<number, Weather["fall"]> = {
    51: rain(0.15),
    53: rain(0.25),
    55: rain(0.35),
    56: rain(0.2),
    57: rain(0.35),
    61: rain(0.4),
    63: rain(0.65),
    65: rain(1),
    66: rain(0.4),
    67: rain(0.8),
    80: rain(0.4),
    81: rain(0.65),
    82: rain(1),
    71: snow(0.3),
    73: snow(0.6),
    75: snow(1),
    77: snow(0.2),
    85: snow(0.5),
    86: snow(1),
    95: rain(1),
    96: rain(1),
    99: rain(1),
  };
  const toward = ((from + 180) * Math.PI) / 180;
  return {
    clouds: Math.min(1, Math.max(0, cloud / 100)),
    fog: code === 45 || code === 48,
    fall: falls[code] ?? null,
    storm: code >= 95,
    wind: { x: Math.sin(toward) * speed, z: -Math.cos(toward) * speed },
  };
}

/** Named weathers, for `fun-preview.html?weather=rain` and screenshots. */
export const PRESETS: Record<string, Weather> = {
  clear: CLEAR,
  cloudy: fromReport({ code: 2, cloud: 50, speed: 5, from: 270 }),
  overcast: fromReport({ code: 3, cloud: 100, speed: 3, from: 270 }),
  fog: fromReport({ code: 45, cloud: 100, speed: 1, from: 270 }),
  drizzle: fromReport({ code: 53, cloud: 100, speed: 2, from: 250 }),
  rain: fromReport({ code: 63, cloud: 100, speed: 4, from: 250 }),
  snow: fromReport({ code: 73, cloud: 100, speed: 2, from: 300 }),
  storm: fromReport({ code: 95, cloud: 100, speed: 9, from: 230 }),
};

const PLACE_KEY = "tethys.fun.place";
const WEATHER_KEY = "tethys.fun.weather";
/** A laptop moves, so where it is is asked again this often. */
const PLACE_TTL_MS = 6 * 60 * 60 * 1000;
/** Open-Meteo's current conditions move on every 15 minutes. */
const POLL_MS = 15 * 60 * 1000;

interface Place {
  lat: number;
  lon: number;
  at: number;
}

function stored<T extends { at: number }>(key: string, ttl: number): T | null {
  try {
    const held = JSON.parse(localStorage.getItem(key) ?? "null") as T | null;
    return held && Date.now() - held.at < ttl ? held : null;
  } catch {
    return null;
  }
}

async function json(url: string): Promise<unknown> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return res.json();
}

async function locate(): Promise<Place> {
  const held = stored<Place>(PLACE_KEY, PLACE_TTL_MS);
  if (held) return held;
  const geo = (await json("https://get.geojs.io/v1/ip/geo.json")) as {
    latitude: string;
    longitude: string;
  };
  const place = {
    lat: Number(geo.latitude),
    lon: Number(geo.longitude),
    at: Date.now(),
  };
  localStorage.setItem(PLACE_KEY, JSON.stringify(place));
  return place;
}

async function lookup(): Promise<Weather> {
  const { lat, lon } = await locate();
  const url =
    "https://api.open-meteo.com/v1/forecast" +
    `?latitude=${lat.toFixed(2)}&longitude=${lon.toFixed(2)}` +
    "&current=weather_code,cloud_cover,wind_speed_10m,wind_direction_10m" +
    "&wind_speed_unit=ms";
  const { current: c } = (await json(url)) as {
    current: {
      weather_code: number;
      cloud_cover: number;
      wind_speed_10m: number;
      wind_direction_10m: number;
    };
  };
  return fromReport({
    code: c.weather_code,
    cloud: c.cloud_cover,
    speed: c.wind_speed_10m,
    from: c.wind_direction_10m,
  });
}

/**
 * The weather where Ryan is, kept fresh while the world is open. The last
 * look is kept in `localStorage`, so a restart opens on it rather than on a
 * clear sky; a failed look keeps whatever was showing.
 */
export function useWeather(fixed?: Weather): Weather {
  const [weather, setWeather] = useState<Weather>(
    () =>
      fixed ??
      stored<{ weather: Weather; at: number }>(WEATHER_KEY, POLL_MS * 4)
        ?.weather ??
      CLEAR,
  );
  useEffect(() => {
    if (fixed) {
      setWeather(fixed);
      return;
    }
    let live = true;
    const look = () =>
      lookup().then(
        (w) => {
          if (!live) return;
          setWeather(w);
          localStorage.setItem(
            WEATHER_KEY,
            JSON.stringify({ weather: w, at: Date.now() }),
          );
        },
        (e) => console.warn("fun mode weather:", e),
      );
    void look();
    const timer = setInterval(look, POLL_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [fixed]);
  return weather;
}

const mix = (a: string, b: string, t: number) =>
  `#${new THREE.Color(a).lerp(new THREE.Color(b), t).getHexString()}`;

/**
 * The world's look under a weather: cloud dims the sun and greys the sky,
 * rain darkens the grass, and snow lies on it.
 */
export function weathered(palette: Palette, w: Weather): Palette {
  const wet = w.fall?.kind === "rain" ? w.fall.rate : 0;
  const settled = w.fall?.kind === "snow" ? 0.5 + w.fall.rate * 0.3 : 0;
  const grey = Math.max(w.clouds * 0.55, w.fog ? 0.6 : 0) + wet * 0.2;
  const sunlit = 1 - w.clouds * 0.6 - wet * 0.15;
  const snowy = (c: string) => mix(c, "#eef3f7", settled);
  return {
    ...palette,
    ground: snowy(mix(palette.ground, "#4f6b45", wet * 0.25)),
    patch: snowy(mix(palette.patch, "#4a6640", wet * 0.25)),
    sky: mix(palette.sky, "#b9c0c8", grey),
    hemiIntensity: palette.hemiIntensity * (1 - grey * 0.15),
    sun: mix(palette.sun, "#e4e8ee", grey),
    sunIntensity: palette.sunIntensity * sunlit,
    foliage: palette.foliage.map((c) => mix(c, "#f4f7fa", settled * 0.4)),
    // Nobody's out with a jar of fireflies in the rain.
    fireflies: palette.fireflies && !w.fall,
  };
}
