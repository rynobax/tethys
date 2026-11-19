import { contextBridge, ipcRenderer, webFrame, webUtils } from "electron";

import type { TethysBridge } from "../src/ipc/bridge";
import { DaemonClient } from "./client";

const SOCKET_ARG = "--tethys-socket=";
const socket = process.argv
  .find((a) => a.startsWith(SOCKET_ARG))
  ?.slice(SOCKET_ARG.length);

const listeners = new Map<string, Set<(payload: unknown) => void>>();
const channels = new Map<number, (message: unknown) => void>();
let nextChannel = 1;

// The connection is the page's: a reload drops it, and the daemon releases
// every PTY subscription it held.
const client = new DaemonClient(socket ?? "", {
  event(name, payload) {
    for (const fn of listeners.get(name) ?? []) {
      try {
        fn(payload);
      } catch (e) {
        console.error(`tethys: "${name}" listener threw:`, e);
      }
    }
  },
  channel(id, message) {
    channels.get(id)?.(message);
  },
  close(reason) {
    console.error(reason);
  },
});
window.addEventListener("pagehide", () => client.close());

// A class rather than bridge API, so the page needs no code for it.
ipcRenderer.on("full-screen", (_event, on: boolean) => {
  document.documentElement.classList.toggle("full-screen", on);
});

// Fun mode's, from the View menu: passed on as a DOM event, so the bridge
// needn't know fun mode exists.
ipcRenderer.on("next-environment", () => {
  window.dispatchEvent(new Event("tethys:next-environment"));
});

// The page's zoom, for what must stay the native size whatever it is: the
// strip under the traffic lights. A zoom resizes the viewport.
const markZoom = () =>
  document.documentElement.style.setProperty(
    "--zoom",
    String(webFrame.getZoomFactor()),
  );
window.addEventListener("DOMContentLoaded", markZoom);
window.addEventListener("resize", markZoom);

const bridge: TethysBridge = {
  invoke: (cmd, args) => client.invoke(cmd, args),
  listen(event, handler) {
    let set = listeners.get(event);
    if (!set) listeners.set(event, (set = new Set()));
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  },
  openChannel(onMessage) {
    const id = nextChannel++;
    channels.set(id, onMessage);
    return id;
  },
  closeChannel(id) {
    channels.delete(id);
  },
  openUrl: (url) => ipcRenderer.invoke("open-url", url),
  pathForFile: (file) => webUtils.getPathForFile(file),
};

contextBridge.exposeInMainWorld("tethys", bridge);
