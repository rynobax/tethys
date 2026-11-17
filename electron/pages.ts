import { net, protocol } from "electron";
import { createReadStream } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";
import { Readable } from "node:stream";
import { pathToFileURL } from "node:url";

/** Must match `convertFileSrc` in `src/ipc/commands.ts`. */
const SCHEME = "tethys-file";

/** Before `ready`: an unprivileged custom scheme can't be framed, and without
 *  `stream` a video waits for the whole file before it plays. */
export function registerPageScheme() {
  protocol.registerSchemesAsPrivileged([
    {
      scheme: SCHEME,
      privileges: {
        standard: true,
        secure: true,
        supportFetchAPI: true,
        stream: true,
      },
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
    const range = request.headers.get("range");
    const partial = range ? await serveRange(file, range) : null;
    return partial ?? fetchFile(file);
  });
}

function fetchFile(file: string) {
  return net.fetch(pathToFileURL(file).toString());
}

/**
 * `net.fetch` answers a `file:` Range with the whole file as a 200, and a
 * video can't seek without a 206. Chromium asks for ranges only for media,
 * and sniffs the container, so the type is a guess. `null` for a multi-range
 * or malformed header, which gets the whole file.
 */
async function serveRange(
  file: string,
  header: string,
): Promise<Response | null> {
  const m = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
  if (!m || (!m[1] && !m[2])) return null;
  const { size } = await fs.stat(file);
  const start = m[1] ? Number(m[1]) : Math.max(0, size - Number(m[2]));
  const end = m[1] && m[2] ? Math.min(Number(m[2]), size - 1) : size - 1;
  if (start > end) {
    return new Response(null, {
      status: 416,
      headers: { "content-range": `bytes */${size}` },
    });
  }
  const body = Readable.toWeb(createReadStream(file, { start, end }));
  const type = MEDIA_TYPES[path.extname(file).toLowerCase()];
  return new Response(body as ReadableStream, {
    status: 206,
    headers: {
      ...(type && { "content-type": type }),
      "content-length": String(end - start + 1),
      "content-range": `bytes ${start}-${end}/${size}`,
      "accept-ranges": "bytes",
    },
  });
}

const MEDIA_TYPES: Record<string, string> = {
  ".mp4": "video/mp4",
  ".m4v": "video/mp4",
  ".mov": "video/quicktime",
  ".webm": "video/webm",
  ".mp3": "audio/mpeg",
  ".m4a": "audio/mp4",
  ".wav": "audio/wav",
};
