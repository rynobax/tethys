import { listen, TauriEvent } from "@tauri-apps/api/event";
import type { Terminal } from "@xterm/xterm";

import * as api from "./ipc/commands";
import { usePtyTerminal } from "./usePtyTerminal";

/** iTerm2's drop format, which Claude Code unescapes and resolves. */
function escapeDroppedPath(p: string): string {
  return p.replace(/([\\ ])/g, "\\$1");
}

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
 * Subscribed once for the app's lifetime and routed to the mounted pane:
 * unlistening soon after `listen()` resolves races Tauri's own registration,
 * throws, and leaks the listener.
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

export function SessionTerminal({ sessionId }: Props) {
  const { containerRef } = usePtyTerminal(sessionId, {
    onReady: (term, container) => wireClaudeExtras(term, container, sessionId),
  });

  return <div className="session-terminal" ref={containerRef} />;
}

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

  // WKWebView pastes a file as an opaque `File` and then auto-inserts its temp
  // path, which xterm bracket-pastes and Claude turns into `[Image #N]`. Keep
  // that for images; for anything else, type the real paths as plain text.
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

  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown") return true;

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
