import { net, protocol } from "electron";
import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

/** Must match `convertFileSrc` in `src/ipc/commands.ts`. */
const SCHEME = "tethys-file";

/** Before `ready`: an unprivileged custom scheme can't be framed. */
export function registerPageScheme() {
  protocol.registerSchemesAsPrivileged([
    {
      scheme: SCHEME,
      privileges: { standard: true, secure: true, supportFetchAPI: true },
    },
  ]);
}

/**
 * Serves artifact Pages, which agents write, so only files under the
 * registry's `worktree_root` (after symlinks) are ever read.
 */
export function servePages(worktreeRoot: () => Promise<string | null>) {
  protocol.handle(SCHEME, async (request) => {
    const root = await worktreeRoot();
    if (!root) return new Response("no worktree_root", { status: 404 });
    const wanted = path.resolve(
      decodeURIComponent(new URL(request.url).pathname),
    );
    let file: string;
    let realRoot: string;
    try {
      [file, realRoot] = await Promise.all([
        fs.realpath(wanted),
        fs.realpath(root),
      ]);
    } catch {
      return new Response("not found", { status: 404 });
    }
    const rel = path.relative(realRoot, file);
    const outside =
      rel === "" ||
      rel === ".." ||
      rel.startsWith(`..${path.sep}`) ||
      path.isAbsolute(rel);
    if (outside) {
      return new Response("outside worktree_root", { status: 403 });
    }
    return net.fetch(pathToFileURL(file).toString());
  });
}
