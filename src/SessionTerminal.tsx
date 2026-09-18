import { listen, TauriEvent } from "@tauri-apps/api/event";
import type { Terminal } from "@xterm/xterm";

import * as api from "./ipc/commands";
import { usePtyTerminal } from "./usePtyTerminal";

/**
 * Backslash-escape spaces in a filesystem path. Matches iTerm2's drop
 * format inside a bracketed paste — Claude Code unescapes `\ ` and resolves
 * the path, which triggers the `[Image #N]` attachment flow for images.
 */
function escapeDroppedPath(p: string): string {
  return p.replace(/([\\ ])/g, "\\$1");
}

/** macOS line/word editing over and above xterm's defaults.
 *
 * Convention: Cmd = whole line, Alt = word. Each row maps a (key, modifier)
 * pair to the readline byte sequence the shell / Claude Code / TUI beneath
 * understands.
 */
type EditBind = { key: string; mod: "cmd" | "alt"; bytes: number[] };
const EDIT_BINDS: EditBind[] = [
  { key: "ArrowLeft", mod: "cmd", bytes: [0x01] }, // Ctrl-A: beginning of line
  { key: "ArrowRight", mod: "cmd", bytes: [0x05] }, // Ctrl-E: end of line
  { key: "Backspace", mod: "cmd", bytes: [0x15] }, // Ctrl-U: kill to start of line
  { key: "Delete", mod: "cmd", bytes: [0x0b] }, // Ctrl-K: kill to end of line
  { key: "ArrowLeft", mod: "alt", bytes: [0x1b, 0x62] }, // Esc-b: previous word
  { key: "ArrowRight", mod: "alt", bytes: [0x1b, 0x66] }, // Esc-f: next word
  { key: "Backspace", mod: "alt", bytes: [0x17] }, // Ctrl-W: backward-kill-word
  { key: "Delete", mod: "alt", bytes: [0x1b, 0x64] }, // Esc-d: kill-word forward
];

/**
 * Drag files from Finder onto the window → paste escaped paths into the
 * active session, like iTerm2. Wrapped in bracketed-paste markers
 * (`\x1b[200~…\x1b[201~`) so Claude Code recognizes it as a paste and runs
 * its path-→-image attachment flow, producing `[Image #N]` for images.
 *
 * The event is window-wide and only one SessionTerminal is mounted at a time,
 * so the subscription is made once for the app's lifetime and routed to
 * whichever pane is current. It used to be per mount, and the unlisten on
 * teardown raced Tauri's own registration: `listen()` resolves with the id
 * before the eval'd script that records the listener has run in the page, so
 * unlistening right after (StrictMode double-mount, a fast workspace switch)
 * threw `listeners[eventId].handlerId` from inside Tauri — as an unhandled
 * rejection, since its unlisten is async — and leaked the listener, so a
 * later drop could paste into the previous workspace's session as well.
 */
type DropTarget = { sessionId: string; term: Terminal };
let dropTarget: DropTarget | null = null;
let dropSubscribed = false;

function subscribeDrops() {
  if (dropSubscribed) return;
  dropSubscribed = true;
  listen<{ paths: string[] }>(TauriEvent.DRAG_DROP, (event) => {
    const target = dropTarget;
    if (!target) return;
    const { paths } = event.payload;
    if (paths.length === 0) return;
    const inner = paths.map(escapeDroppedPath).join(" ") + " ";
    api
      .sendInput(
        target.sessionId,
        Array.from(new TextEncoder().encode(`\x1b[200~${inner}\x1b[201~`)),
      )
      .catch((e) => console.error("send_input (drag-drop) failed:", e));
    target.term.focus();
  }).catch((e) => {
    dropSubscribed = false;
    console.error("drag-drop subscribe failed:", e);
  });
}

interface Props {
  sessionId: string;
}

/**
 * xterm.js surface for a Claude session.
 *
 * The pane lifecycle — construction, attach, streaming, resize, teardown —
 * lives in `usePtyTerminal`. What's left here is what only a Claude session
 * needs: Finder paste interception, macOS editing keybinds, and drag-drop.
 */
export function SessionTerminal({ sessionId }: Props) {
  const { containerRef } = usePtyTerminal(sessionId, {
    onReady: (term, container) => wireClaudeExtras(term, container, sessionId),
  });

  return <div className="session-terminal" ref={containerRef} />;
}

/**
 * The three Claude-specific behaviours, and their teardown.
 */
function wireClaudeExtras(
  term: Terminal,
  container: HTMLDivElement,
  sessionId: string,
) {
  const sendRaw = (bytes: number[], what: string) => {
    api.sendInput(sessionId, bytes).catch((e) => {
      console.error(`send_input (${what}) failed:`, e);
    });
  };

  // Cmd+V of a file from Finder/screenshot: WKWebView delivers only an
  // opaque `File` (no `text/plain`, no `text/uri-list`) and then quietly
  // auto-inserts the temp path into the helper textarea after the paste
  // event. xterm wraps that text in bracketed-paste markers, which trips
  // Claude Code's path-→-image flow indiscriminately — turning a pasted log
  // path into `[Image #N]`.
  //
  // For image MIME we want that flow (it's the whole point of pasting a
  // screenshot). For everything else we want iTerm2-style behavior: the path
  // appears as plain typed text. Branch on file MIME, intercept the non-image
  // case, read real paths from NSPasteboard via Rust, and inject raw bytes
  // without bracketed-paste markers.
  const helperTextarea = container.querySelector<HTMLTextAreaElement>(
    ".xterm-helper-textarea",
  );
  const onPaste = (ev: ClipboardEvent) => {
    const cd = ev.clipboardData;
    if (!cd || cd.files.length === 0) return;
    const allImages = Array.from(cd.files).every((f) =>
      f.type.startsWith("image/"),
    );
    if (allImages) return;
    ev.preventDefault();
    ev.stopImmediatePropagation();
    api
      .readClipboardFilePaths()
      .then((paths) => {
        if (paths.length === 0) return;
        const text = paths.map(escapeDroppedPath).join(" ") + " ";
        return api.sendInput(
          sessionId,
          Array.from(new TextEncoder().encode(text)),
        );
      })
      .catch((e) => console.error("file paste failed:", e));
  };
  helperTextarea?.addEventListener("paste", onPaste, true);

  // Returning false suppresses xterm's default dispatch for that key; we send
  // our own byte sequence instead.
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown") return true;

    // Shift+Enter → newline (Option+Enter equivalent in Claude Code).
    if (
      ev.key === "Enter" &&
      ev.shiftKey &&
      !ev.metaKey &&
      !ev.altKey &&
      !ev.ctrlKey
    ) {
      ev.preventDefault();
      sendRaw([0x1b, 0x0d], "shift-enter");
      return false;
    }

    const onlyCmd = ev.metaKey && !ev.altKey && !ev.ctrlKey && !ev.shiftKey;
    const onlyAlt = ev.altKey && !ev.metaKey && !ev.ctrlKey && !ev.shiftKey;
    for (const { key, mod, bytes } of EDIT_BINDS) {
      if (ev.key !== key) continue;
      if (mod === "cmd" && !onlyCmd) continue;
      if (mod === "alt" && !onlyAlt) continue;
      ev.preventDefault();
      sendRaw(bytes, "keybind");
      return false;
    }

    return true;
  });

  subscribeDrops();
  const target: DropTarget = { sessionId, term };
  dropTarget = target;

  return () => {
    if (dropTarget === target) dropTarget = null;
    helperTextarea?.removeEventListener("paste", onPaste, true);
  };
}
