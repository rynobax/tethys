import { useEffect, useRef } from "react";
import * as api from "./ipc/commands";
import { Channel } from "./ipc/commands";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ClipboardAddon } from "@xterm/addon-clipboard";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";
import { themeToXterm, useTheme } from "./theme";

const DEFAULT_XTERM_THEME = {
  background: "#0a0a0a",
  foreground: "#e8e8e8",
};

/**
 * Backslash-escape spaces in a filesystem path. Matches iTerm2's drop
 * format inside a bracketed paste — Claude Code unescapes `\ ` and resolves
 * the path, which triggers the `[Image #N]` attachment flow for images.
 */
function escapeDroppedPath(p: string): string {
  return p.replace(/([\\ ])/g, "\\$1");
}

interface Props {
  sessionId: string;
}

/**
 * xterm.js surface for a Tethys PTY session. On mount:
 *   1. Create terminal + canvas/fit/clipboard addons.
 *   2. Create a raw-bytes `Channel`.
 *   3. Call `attach_session` — returns historical scrollback, registers the
 *      channel for live fan-out.
 *   4. Write scrollback into xterm, then drain the channel straight into it.
 * Keystrokes go via `send_input`, resize events via `resize_session`.
 */
export function SessionTerminal({ sessionId }: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const theme = useTheme();
  // Snapshot the current theme for the mount-time init so the main useEffect
  // doesn't need `theme` as a dep (which would rebuild xterm on every change).
  const themeRef = useRef(theme);
  themeRef.current = theme;

  useEffect(() => {
    if (!termRef.current) return;
    const next = theme ? themeToXterm(theme) : DEFAULT_XTERM_THEME;
    termRef.current.options.theme = next;
  }, [theme]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const initialTheme = themeRef.current
      ? themeToXterm(themeRef.current)
      : DEFAULT_XTERM_THEME;
    const term = new Terminal({
      fontFamily: '"SF Mono", ui-monospace, Menlo, monospace',
      fontSize: 16,
      theme: initialTheme,
      cursorBlink: true,
      // Fallback scrollback for panes that don't capture the mouse (plain
      // shell, Claude's classic renderer). Claude's fullscreen renderer
      // requests SGR mouse tracking, so there the wheel is forwarded to
      // the app and Claude scrolls its own transcript instead.
      scrollback: 50000,
      // With mouse tracking active, xterm.js hands drags to the app rather
      // than selecting. Option+drag forces a native xterm selection —
      // matches iTerm2, and gives an escape hatch when Claude owns the
      // mouse (it does its own selection + pbcopy, but this covers the
      // cases its selection can't reach).
      macOptionClickForcesSelection: true,
      allowProposedApi: true,
      // OSC 8 escape-sequence hyperlinks (Claude Code emits these for PR
      // URLs etc.) default to `window.open` which Tauri's WKWebView blocks
      // and routes through dialog.confirm. Route through plugin-opener.
      linkHandler: {
        activate: (_ev, uri) => {
          openUrl(uri).catch((e) => {
            console.error("openUrl failed:", e);
          });
        },
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new ClipboardAddon());
    // Ctrl/Cmd+Click a URL → open in the default browser. We route through
    // plugin-opener so WKWebView doesn't try to intercept navigation and
    // prompt via `dialog.confirm` (which isn't in our capability set).
    term.loadAddon(
      new WebLinksAddon((event, uri) => {
        event.preventDefault();
        openUrl(uri).catch((e) => {
          console.error("openUrl failed:", e);
        });
      }),
    );
    // Using xterm's default DOM renderer — @xterm/addon-canvas reaches into
    // v5 internals that v6 removed (`_linkifier2`), and WebGL + WKWebView
    // has known issues on macOS. DOM is plenty fast for interactive shells.
    term.open(container);
    fit.fit();
    term.focus();

    // Cmd+V of a file from Finder/screenshot: WKWebView delivers only an
    // opaque `File` (no `text/plain`, no `text/uri-list`) and then quietly
    // auto-inserts the temp path into the helper textarea after the paste
    // event. xterm wraps that text in bracketed-paste markers, which trips
    // Claude Code's path-→-image flow indiscriminately — turning a pasted
    // log path into `[Image #N]`.
    //
    // For image MIME we want that flow (it's the whole point of pasting a
    // screenshot). For everything else we want iTerm2-style behavior: the
    // path appears as plain typed text. Branch on file MIME, intercept the
    // non-image case, read real paths from NSPasteboard via Rust, and inject
    // raw bytes without bracketed-paste markers.
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
      api.readClipboardFilePaths()
        .then((paths) => {
          if (paths.length === 0) return;
          const text = paths.map(escapeDroppedPath).join(" ") + " ";
          const bytes = Array.from(new TextEncoder().encode(text));
          return api.sendInput(sessionId, bytes);
        })
        .catch((e) => {
          console.error("file paste failed:", e);
        });
    };
    helperTextarea?.addEventListener("paste", onPaste, true);

    // macOS-friendly keybindings over and above xterm's defaults.
    // Returning false suppresses xterm's default dispatch for that key;
    // we send our own byte sequence via send_input.
    const sendRaw = (bytes: number[]) => {
      api.sendInput(sessionId, bytes).catch((e) => {
        console.error("send_input (keybind) failed:", e);
      });
    };
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
        sendRaw([0x1b, 0x0d]);
        return false;
      }

      // macOS line/word editing. Convention: Cmd = whole line, Alt = word.
      // Each row maps a (key + modifier) pair to a readline byte sequence
      // that the underlying shell / Claude Code / TUI app understands.
      const onlyCmd = ev.metaKey && !ev.altKey && !ev.ctrlKey && !ev.shiftKey;
      const onlyAlt = ev.altKey && !ev.metaKey && !ev.ctrlKey && !ev.shiftKey;
      type EditBind = { key: string; mod: "cmd" | "alt"; bytes: number[] };
      const edits: EditBind[] = [
        // Cmd → line operations.
        { key: "ArrowLeft", mod: "cmd", bytes: [0x01] }, // Ctrl-A: beginning of line
        { key: "ArrowRight", mod: "cmd", bytes: [0x05] }, // Ctrl-E: end of line
        { key: "Backspace", mod: "cmd", bytes: [0x15] }, // Ctrl-U: kill to start of line
        { key: "Delete", mod: "cmd", bytes: [0x0b] }, // Ctrl-K: kill to end of line
        // Alt → word operations (the bindings Cmd used to do).
        { key: "ArrowLeft", mod: "alt", bytes: [0x1b, 0x62] }, // Esc-b: previous word
        { key: "ArrowRight", mod: "alt", bytes: [0x1b, 0x66] }, // Esc-f: next word
        { key: "Backspace", mod: "alt", bytes: [0x17] }, // Ctrl-W: backward-kill-word
        { key: "Delete", mod: "alt", bytes: [0x1b, 0x64] }, // Esc-d: kill-word forward
      ];
      for (const { key, mod, bytes } of edits) {
        if (ev.key !== key) continue;
        if (mod === "cmd" && !onlyCmd) continue;
        if (mod === "alt" && !onlyAlt) continue;
        ev.preventDefault();
        sendRaw(bytes);
        return false;
      }

      return true;
    });

    termRef.current = term;
    fitRef.current = fit;

    // Keystrokes → backend.
    const dataSub = term.onData((data) => {
      const bytes = Array.from(new TextEncoder().encode(data));
      api.sendInput(sessionId, bytes).catch((e) => {
        console.error("send_input failed:", e);
      });
    });

    // Resize → backend.
    const resizeSub = term.onResize(({ cols, rows }) => {
      api.resizeSession(sessionId, cols, rows).catch((e) => {
        console.error("resize_session failed:", e);
      });
    });

    // Attach: get scrollback + start streaming.
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (chunk) => {
      term.write(new Uint8Array(chunk));
    };

    let cancelled = false;
    api.attachSession(sessionId, channel)
      .then((scrollback) => {
        if (cancelled) return;
        if (scrollback.length > 0) {
          term.write(new Uint8Array(scrollback));
        }
        // Final resize to let the backend match xterm's cols/rows right away.
        const { cols, rows } = term;
        api.resizeSession(sessionId, cols, rows).catch(() => {});
      })
      .catch((e) => {
        term.write(`\r\n\x1b[31m[attach failed: ${String(e)}]\x1b[0m\r\n`);
      });

    // Fit on container resize.
    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // xterm throws if the container has zero size (e.g. during transition)
      }
    });
    ro.observe(container);

    // Drag files from Finder onto the window → paste escaped paths into the
    // active session, like iTerm2. Wrapped in bracketed-paste markers
    // (`\x1b[200~…\x1b[201~`) so Claude Code recognizes it as a paste and
    // runs its path-→-image attachment flow, producing `[Image #N]` for
    // images. The event is window-wide; only one SessionTerminal mounts at
    // a time, so no per-pane gating is needed.
    let dragUnlisten: (() => void) | null = null;
    let dragDisposed = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        if (event.payload.paths.length === 0) return;
        const inner =
          event.payload.paths.map(escapeDroppedPath).join(" ") + " ";
        const text = `\x1b[200~${inner}\x1b[201~`;
        const bytes = Array.from(new TextEncoder().encode(text));
        api.sendInput(sessionId, bytes).catch((e) => {
          console.error("send_input (drag-drop) failed:", e);
        });
        term.focus();
      })
      .then((fn) => {
        if (dragDisposed) {
          try {
            fn();
          } catch {}
        } else {
          dragUnlisten = fn;
        }
      })
      .catch((e) => {
        console.error("onDragDropEvent subscribe failed:", e);
      });

    return () => {
      cancelled = true;
      ro.disconnect();
      dataSub.dispose();
      resizeSub.dispose();
      dragDisposed = true;
      try {
        dragUnlisten?.();
      } catch {}
      helperTextarea?.removeEventListener("paste", onPaste, true);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      // Stop the backend fanning bytes to this dead pane. Its retain-on-error
      // path never fires on its own: the channel keeps succeeding as long as
      // its JS callback is registered, so we must detach explicitly.
      api.detachSession(sessionId, channel.id).catch(
        () => {},
      );
      // Sever the channel's reference to `term`. Tauri registers the channel's
      // callback in a global registry and only unregisters it on an end-of-
      // stream message, which command-arg channels never send — so the closure
      // would otherwise pin the whole disposed terminal (and its 50k-line
      // scrollback) in the webview forever.
      channel.onmessage = () => {};
    };
  }, [sessionId]);

  return <div className="session-terminal" ref={containerRef} />;
}
