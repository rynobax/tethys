import { createContext, useContext } from "react";
import type { Theme } from "./types";

export interface XtermTheme {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

export function themeToXterm(theme: Theme): XtermTheme {
  const a = theme.colors.ansi;
  return {
    background: theme.colors.background,
    foreground: theme.colors.foreground,
    cursor: theme.colors.cursor,
    cursorAccent: theme.colors.cursor_text,
    selectionBackground: theme.colors.selection,
    black: a[0],
    red: a[1],
    green: a[2],
    yellow: a[3],
    blue: a[4],
    magenta: a[5],
    cyan: a[6],
    white: a[7],
    brightBlack: a[8],
    brightRed: a[9],
    brightGreen: a[10],
    brightYellow: a[11],
    brightBlue: a[12],
    brightMagenta: a[13],
    brightCyan: a[14],
    brightWhite: a[15],
  };
}

export function themeToCssVars(theme: Theme): Record<string, string> {
  const c = theme.colors;
  const a = c.ansi;
  return {
    "--bg": c.background,
    "--fg": c.foreground,
    "--muted": a[8],
    "--border": `color-mix(in oklab, ${a[8]} 40%, transparent)`,
    "--sidebar-bg": `color-mix(in oklab, ${c.background} 94%, ${c.foreground} 6%)`,
    "--accent": a[4],
    "--accent-fg": c.background,
    "--danger": a[1],
    "--selected-bg": c.selection,
    "--turn-dot": a[3],
  };
}

export function applyTheme(theme: Theme | null) {
  const root = document.documentElement;
  const keys = [
    "--bg",
    "--fg",
    "--muted",
    "--border",
    "--sidebar-bg",
    "--accent",
    "--accent-fg",
    "--danger",
    "--selected-bg",
    "--turn-dot",
  ];
  if (!theme) {
    for (const k of keys) root.style.removeProperty(k);
    return;
  }
  const vars = themeToCssVars(theme);
  for (const [k, v] of Object.entries(vars)) {
    root.style.setProperty(k, v);
  }
}

export const ThemeContext = createContext<Theme | null>(null);

export function useTheme(): Theme | null {
  return useContext(ThemeContext);
}
