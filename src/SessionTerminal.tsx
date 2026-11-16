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

/** Drops anywhere in the window go to the shown terminal, as in a terminal
 *  app. Listened for once, at the document, so a file dropped elsewhere can't
 *  navigate the window to it either. */
type DropTarget = { sessionId: string; term: Terminal };
let dropTarget: DropTarget | null = null;
let dropSubscribed = false;

const carriesFiles = (ev: DragEvent) =>
  ev.dataTransfer?.types.includes("Files") ?? false;

function subscribeDrops() {
  if (dropSubscribed) return;
  dropSubscribed = true;
  document.addEventListener("dragover", (ev) => {
    if (carriesFiles(ev)) ev.preventDefault();
  });
  document.addEventListener("drop", (ev) => {
    if (!carriesFiles(ev)) return;
    ev.preventDefault();
    const target = dropTarget;
    if (!target) return;
    const paths = Array.from(ev.dataTransfer!.files)
      .map(api.pathForFile)
      .filter((p) => p !== "");
    if (paths.length === 0) return;
    const inner = paths.map(escapeDroppedPath).join(" ") + " ";
    api
      .sendInput(
        target.sessionId,
        Array.from(new TextEncoder().encode(`\x1b[200~${inner}\x1b[201~`)),
      )
      .catch((e) => console.error("send_input (drag-drop) failed:", e));
    target.term.focus();
  });
}

interface Props {
  sessionId: string;
}

export function SessionTerminal({ sessionId }: Props) {
  const containerRef = usePtyTerminal(sessionId, (term, container) =>
    wireClaudeExtras(term, container, sessionId),
  );

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

  // Claude turns a bracket-pasted image path into `[Image #N]`, so image files
  // are pasted that way; other files are typed as plain paths. Image data with
  // no file behind it (a screenshot) has no path at all, and Chromium pastes
  // nothing for it, so the agent is sent Ctrl-V to read the clipboard itself.
  const helperTextarea = container.querySelector<HTMLTextAreaElement>(
    ".xterm-helper-textarea",
  );
  const onPaste = (ev: ClipboardEvent) => {
    const cd = ev.clipboardData;
    if (!cd || cd.files.length === 0) return;
    ev.preventDefault();
    ev.stopImmediatePropagation();
    const files = Array.from(cd.files);
    if (files.every((f) => f.type.startsWith("image/"))) {
      const paths = files.map(api.pathForFile).filter((p) => p !== "");
      if (paths.length === 0) {
        sendRaw([0x16], "image paste");
        return;
      }
      const inner = paths.map(escapeDroppedPath).join(" ");
      sendRaw(
        Array.from(new TextEncoder().encode(`\x1b[200~${inner}\x1b[201~`)),
        "image paste",
      );
      return;
    }
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
