import net from "node:net";
import os from "node:os";
import path from "node:path";

/** Where `tethysd` keeps everything, `ui.sock` included. */
export function dataDir(): string {
  return (
    process.env.TETHYS_DATA_DIR ||
    path.join(os.homedir(), "Library/Application Support/app.tethys.dev")
  );
}

export const socketPath = () => path.join(dataDir(), "ui.sock");

const KIND_JSON = 0;
const KIND_BYTES = 1;

interface Handlers {
  event?: (name: string, payload: unknown) => void;
  /** JSON messages arrive parsed; binary frames as a `Uint8Array` copy. */
  channel?: (id: number, message: unknown) => void;
  close?: (reason: string) => void;
}

/**
 * One connection to `tethysd`'s UI socket. Everything it subscribed to is
 * released by the daemon when it closes, so its lifetime is the subscriber's.
 */
export class DaemonClient {
  private readonly socket: net.Socket;
  private nextId = 1;
  private readonly pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: unknown) => void }
  >();
  private buffer = Buffer.alloc(0);
  private closed: string | null = null;

  constructor(
    sockPath: string,
    private readonly handlers: Handlers = {},
  ) {
    this.socket = net.createConnection(sockPath);
    this.socket.on("data", (chunk) => this.onData(chunk as Buffer));
    this.socket.on("error", (e) => this.shut(`Tethys daemon: ${e.message}`));
    this.socket.on("close", () =>
      this.shut("Tethys daemon closed the connection"),
    );
  }

  invoke(cmd: string, args?: unknown): Promise<unknown> {
    if (this.closed) return Promise.reject(this.closed);
    const id = this.nextId++;
    const request: Record<string, unknown> = { id, cmd };
    if (args !== undefined) request.args = args;
    const body = Buffer.from(JSON.stringify(request), "utf8");
    const frame = Buffer.alloc(5 + body.length);
    frame.writeUInt32BE(1 + body.length, 0);
    frame[4] = KIND_JSON;
    body.copy(frame, 5);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.write(frame);
    });
  }

  close() {
    this.socket.destroy();
  }

  private shut(reason: string) {
    if (this.closed) return;
    this.closed = reason;
    for (const { reject } of this.pending.values()) reject(reason);
    this.pending.clear();
    this.handlers.close?.(reason);
  }

  private onData(chunk: Buffer) {
    this.buffer =
      this.buffer.length === 0 ? chunk : Buffer.concat([this.buffer, chunk]);
    while (this.buffer.length >= 4) {
      const len = this.buffer.readUInt32BE(0);
      if (this.buffer.length < 4 + len) return;
      const frame = this.buffer.subarray(4, 4 + len);
      this.buffer = this.buffer.subarray(4 + len);
      try {
        this.onFrame(frame);
      } catch (e) {
        console.error("tethys: bad frame from daemon:", e);
      }
    }
  }

  private onFrame(frame: Buffer) {
    if (frame.length === 0) return;
    if (frame[0] === KIND_BYTES) {
      const channel = frame.readUInt32BE(1);
      // Copied, so a held chunk doesn't pin the whole read it came in.
      this.handlers.channel?.(channel, new Uint8Array(frame.subarray(5)));
      return;
    }
    const msg = JSON.parse(frame.subarray(1).toString("utf8"));
    if (typeof msg.id === "number") {
      const waiter = this.pending.get(msg.id);
      if (!waiter) return;
      this.pending.delete(msg.id);
      if ("err" in msg) waiter.reject(msg.err);
      else waiter.resolve(msg.ok);
    } else if (typeof msg.event === "string") {
      this.handlers.event?.(msg.event, msg.payload);
    } else if (typeof msg.channel === "number") {
      this.handlers.channel?.(msg.channel, msg.payload);
    }
  }
}
