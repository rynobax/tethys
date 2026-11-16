// Bundles main and preload for Electron. The preload can't import the app's
// ESM, and a bundle means the packaged app needs no node_modules.
import { build } from "esbuild";

await build({
  entryPoints: { main: "electron/main.ts", preload: "electron/preload.ts" },
  outdir: "dist/electron",
  outExtension: { ".js": ".cjs" },
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node22",
  external: ["electron"],
  sourcemap: "inline",
  logLevel: "warning",
});
