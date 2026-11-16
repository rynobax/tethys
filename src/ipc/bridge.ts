/**
 * What the preload (`electron/preload.ts`) exposes as `window.tethys`. Only
 * `ipc/commands.ts` uses it; fun mode's preview installs a stand-in.
 */
export interface TethysBridge {
  /** Rejects with the daemon's error value unchanged, a display string. */
  invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown>;
  /** Returns the unlisten function. */
  listen(event: string, handler: (payload: unknown) => void): () => void;
  /** A channel id unique to this page's connection. Binary frames arrive as
   *  `Uint8Array`s, JSON ones parsed. */
  openChannel(onMessage: (message: unknown) => void): number;
  closeChannel(id: number): void;
  /** http(s) only; anything else is dropped. */
  openUrl(url: string): Promise<void>;
  /** The absolute path of a dropped `File`. */
  pathForFile(file: File): string;
}

declare global {
  interface Window {
    tethys: TethysBridge;
  }
}
