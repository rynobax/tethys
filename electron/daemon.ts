import { spawn, type ChildProcess } from "node:child_process";
import path from "node:path";
import readline from "node:readline";

/** `tethys-hook` and `tethys-mcp` must sit beside it: it finds them there. */
export const daemonBinary = () =>
  process.env.TETHYS_DAEMON ||
  path.join(path.dirname(process.execPath), "tethysd");

/**
 * Resolves once `tethysd` prints `ready`, i.e. its socket is listening. Its
 * stdin is a pipe nothing writes to and only this process's exit closes: the
 * daemon quits on EOF, which is how it dies with the app.
 */
export function startDaemon(
  onExit: (code: number | null, signal: string | null) => void,
): Promise<ChildProcess> {
  const binary = daemonBinary();
  const child = spawn(binary, [], {
    stdio: ["pipe", "pipe", "pipe"],
    env: process.env,
  });
  // Passed through, keeping the last line to explain an exit before ready
  // (another Tethys serving the data dir, say).
  let lastErr = "";
  readline.createInterface({ input: child.stderr! }).on("line", (line) => {
    process.stderr.write(`${line}\n`);
    if (line.trim()) lastErr = line;
  });
  return new Promise((resolve, reject) => {
    let ready = false;
    readline.createInterface({ input: child.stdout! }).on("line", (line) => {
      if (!ready && line.trim() === "ready") {
        ready = true;
        resolve(child);
      } else {
        console.log(`tethysd: ${line}`);
      }
    });
    child.on("error", (e) => reject(new Error(`${binary}: ${e.message}`)));
    // `close`, not `exit`, so stderr has been read to the end.
    child.on("close", (code, signal) => {
      if (ready) onExit(code, signal);
      else
        reject(
          new Error(
            lastErr ||
              `${binary} exited before it was ready (${signal ?? `code ${code}`})`,
          ),
        );
    });
  });
}
