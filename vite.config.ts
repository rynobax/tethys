import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
// @ts-expect-error process is a nodejs global
const frozen = !!process.env.TETHYS_NO_HMR;

/**
 * `dev:frozen` hot-reloads fun mode and nothing else. Every module under
 * `src/fun/` but `FunToggle` stops at a React Refresh boundary inside
 * `src/fun/` (`FunWorld`, `Leash`), so an edit there re-renders the world and
 * can't re-run the app around it. Any other change is dropped, as frozen has
 * always done. A dependency or config change still restarts Vite, which
 * reloads the page.
 */
function funModeOnlyHmr(): Plugin {
  return {
    name: "tethys:fun-mode-only-hmr",
    apply: "serve",
    handleHotUpdate({ file }) {
      const fun =
        file.includes("/src/fun/") && !file.endsWith("/src/fun/FunToggle.tsx");
      return fun ? undefined : [];
    },
  };
}

export default defineConfig(async () => ({
  plugins: [react(), ...(frozen ? [funModeOnlyHmr()] : [])],
  // Bundled up front: found on first opening fun mode instead, Vite would
  // re-optimize and reload the page, which frozen exists to prevent.
  optimizeDeps: { include: ["three", "@react-three/fiber"] },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
