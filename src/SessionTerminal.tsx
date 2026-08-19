import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { Terminal } from "@xterm/xterm";

import * as api from "./ipc/commands";
import { sessionEndpoint, usePtyTerminal } from "./usePtyTerminal";

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
  const { containerRef } = usePtyTerminal(sessionId, sessionEndpoint, {
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

  // Drag files from Finder onto the window → paste escaped paths into the
  // active session, like iTerm2. Wrapped in bracketed-paste markers
  // (`\x1b[200~…\x1b[201~`) so Claude Code recognizes it as a paste and runs
  // its path-→-image attachment flow, producing `[Image #N]` for images. The
  // event is window-wide; only one SessionTerminal mounts at a time, so no
  // per-pane gating is needed.
  let dragUnlisten: (() => void) | null = null;
  let dragDisposed = false;
  getCurrentWebview()
    .onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      if (event.payload.paths.length === 0) return;
      const inner = event.payload.paths.map(escapeDroppedPath).join(" ") + " ";
      sendRaw(
        Array.from(new TextEncoder().encode(`\x1b[200~${inner}\x1b[201~`)),
        "drag-drop",
      );
      term.focus();
    })
    .then((fn) => {
      if (dragDisposed) {
        try {
          fn();
        } catch {
          // Already torn down; nothing to release.
        }
      } else {
        dragUnlisten = fn;
      }
    })
    .catch((e) => console.error("onDragDropEvent subscribe failed:", e));

  return () => {
    dragDisposed = true;
    try {
      dragUnlisten?.();
    } catch {
      // Best-effort: the webview may already be gone.
    }
    helperTextarea?.removeEventListener("paste", onPaste, true);
  };
}
