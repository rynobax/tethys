import { contextBridge, ipcRenderer, webUtils } from "electron";

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
