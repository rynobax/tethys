import { app, BrowserWindow, dialog, ipcMain, Menu } from "electron";
import type { ChildProcess } from "node:child_process";
import path from "node:path";

import { DaemonClient, dataDir, socketPath } from "./client";
import { popupContextMenu } from "./context-menu";
import { startDaemon } from "./daemon";
import { openExternal } from "./open-external";
import { registerPageScheme, servePages } from "./pages";
import { policePrViews, watchPrWrites } from "./pr-view";

// A test run's data dir holds Electron's profile too, so it never shares the
// single-instance lock or the GitHub login with real use.
if (process.env.TETHYS_DATA_DIR) {
  app.setPath("userData", path.join(dataDir(), "electron"));
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  registerPageScheme();
  app.on("second-instance", () => {
    const win = BrowserWindow.getAllWindows()[0];
    if (!win) return;
    if (win.isMinimized()) win.restore();
    win.show();
    win.focus();
  });
  app.on("window-all-closed", () => app.quit());
  app.on("will-quit", stopDaemon);
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.on(signal, () => app.quit());
  }
  app.whenReady().then(boot).catch(fail);
}

let daemonProcess: ChildProcess | null = null;
let quitting = false;

/** Waits for the daemon to go, so a restarted app never finds it still
 *  holding its sockets. Closing stdin is its signal. */
function stopDaemon(event: Electron.Event) {
  quitting = true;
  const child = daemonProcess;
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  event.preventDefault();
  daemonProcess = null;
  const done = () => app.exit(0);
  child.once("exit", done);
  setTimeout(done, 5000).unref();
  child.stdin?.end();
}

function fail(e: unknown): never {
  dialog.showErrorBox("Tethys couldn't start", String(e));
  app.exit(1);
  throw e;
}

async function boot() {
  daemonProcess = await startDaemon((code, signal) => {
    if (quitting) return;
    dialog.showErrorBox(
      "Tethys daemon stopped",
      `tethysd exited (${signal ?? `code ${code}`}).`,
    );
    app.exit(1);
  }).catch(fail);

  // The shell's own connection: the menu, window focus and Pages' scope.
  const daemon = new DaemonClient(socketPath());
  const call = (cmd: string, args?: unknown) =>
    daemon.invoke(cmd, args).catch((e) => console.error(`${cmd}:`, e));

  let worktreeRoot: string | null = null;
  servePages(async () => {
    if (worktreeRoot) return worktreeRoot;
    const status = (await daemon
      .invoke("registry_status")
      .catch(() => null)) as {
      kind: string;
      registry?: { worktree_root: string };
    } | null;
    worktreeRoot =
      status?.kind === "ok" ? status.registry!.worktree_root : null;
    return worktreeRoot;
  });

  const win = createWindow();
  installMenu(win, call);
  win.on("focus", () => call("request_github_poll"));
  watchPrWrites((url, cause) => call("poll_pr", { url, cause }));
}

function createWindow(): BrowserWindow {
  // A test run never shows the window, so it can't take the user's focus.
  const hidden = !!process.env.TETHYS_TEST;
  const win = new BrowserWindow({
    title: "Tethys",
    width: 1100,
    height: 720,
    minWidth: 700,
    minHeight: 480,
    show: false,
    // The page draws its own strip under the traffic lights (`.titlebar`), so
    // the title bar takes the theme's colour.
    titleBarStyle: "hidden",
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      // The preload is the daemon client and needs Node's `net`.
      sandbox: false,
      nodeIntegration: false,
      webviewTag: true,
      additionalArguments: [`--tethys-socket=${socketPath()}`],
    },
  });

  const contents = win.webContents;
  contents.setWindowOpenHandler(({ url }) => {
    openExternal(url);
    return { action: "deny" };
  });
  // The app page only ever reloads, which counts as a navigation here; a
  // dropped file would otherwise replace it.
  const sansHash = (u: string) => u.split("#")[0];
  contents.on("will-navigate", (event) => {
    if (sansHash(event.url) !== sansHash(contents.getURL())) {
      event.preventDefault();
    }
  });
  policePrViews(contents);
  // Covers artifact frames too. Where the page handles a right-click itself
  // (sidebar rows), this never fires.
  contents.on("context-menu", (_event, params) => {
    popupContextMenu(contents, params, { window: win, navigation: false });
  });
  // Fullscreen hides the traffic lights, so the strip goes too.
  const sendFullScreen = () => contents.send("full-screen", win.isFullScreen());
  win.on("enter-full-screen", sendFullScreen);
  win.on("leave-full-screen", sendFullScreen);
  contents.on("dom-ready", sendFullScreen);
  ipcMain.handle("open-url", (event, url: unknown) => {
    if (event.sender === contents && typeof url === "string") openExternal(url);
  });

  if (!hidden) {
    win.once("ready-to-show", () => {
      win.maximize();
      win.show();
    });
  }

  const devUrl = process.env.TETHYS_DEV_URL;
  if (devUrl) win.loadURL(devUrl);
  else win.loadFile(path.join(__dirname, "../renderer/index.html"));
  return win;
}

function installMenu(
  win: BrowserWindow,
  call: (cmd: string, args?: unknown) => Promise<unknown>,
) {
  const loadTheme = async () => {
    const picked = await dialog.showOpenDialog(win, {
      filters: [{ name: "iTerm2 colors", extensions: ["itermcolors"] }],
      properties: ["openFile"],
    });
    if (!picked.canceled && picked.filePaths[0]) {
      await call("load_theme", { path: picked.filePaths[0] });
    }
  };
  Menu.setApplicationMenu(
    Menu.buildFromTemplate([
      { role: "appMenu" },
      { role: "fileMenu" },
      { role: "editMenu" },
      {
        label: "View",
        submenu: [
          { role: "reload" },
          { role: "forceReload" },
          { role: "toggleDevTools" },
          { type: "separator" },
          { role: "resetZoom" },
          { role: "zoomIn" },
          { role: "zoomOut" },
          { type: "separator" },
          { role: "togglefullscreen" },
          { type: "separator" },
          { label: "Load .itermcolors…", click: () => void loadTheme() },
          {
            label: "Reset theme to default",
            click: () => void call("reset_theme"),
          },
        ],
      },
      { role: "windowMenu" },
    ]),
  );
}
