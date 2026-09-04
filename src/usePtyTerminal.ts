import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ClipboardAddon } from "@xterm/addon-clipboard";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";

import * as api from "./ipc/commands";
import { Channel } from "./ipc/commands";
import { themeToXterm, useTheme } from "./theme";

const DEFAULT_XTERM_THEME = {
  background: "#0a0a0a",
  foreground: "#e8e8e8",
};

export interface PtyTerminalOptions {
  /**
   * Extra wiring that needs the live `Terminal` — the Claude-specific paste,
   * keybinding and drag-drop handlers. Returns its own teardown, which runs
   * before the terminal is disposed.
   *
   * Captured in a ref, so it doesn't have to be stable and doesn't rebuild
   * the terminal when it changes.
   */
  onReady?: (term: Terminal, container: HTMLDivElement) => (() => void) | void;
}

/**
 * Mounts an xterm.js terminal bound to a session's PTY.
 *
 * Owns the whole lifecycle: terminal construction with its three addons and
 * link handler, keystroke and resize forwarding, the attach → scrollback →
 * drain sequence, `ResizeObserver`-driven fitting, and the teardown —
 * including the detach-then-sever-the-channel step whose absence was a real
 * memory leak.
 *
 * `sessionId` identifies the pane; changing it rebuilds the terminal.
 */
export function usePtyTerminal(
  sessionId: string,
  options: PtyTerminalOptions = {},
) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const theme = useTheme();
  // Snapshot the theme for mount-time init so the main effect doesn't take
  // `theme` as a dep, which would rebuild xterm on every theme change.
  const themeRef = useRef(theme);
  themeRef.current = theme;
  const onReadyRef = useRef(options.onReady);
  onReadyRef.current = options.onReady;

  useEffect(() => {
    if (!termRef.current) return;
    termRef.current.options.theme = theme
      ? themeToXterm(theme)
      : DEFAULT_XTERM_THEME;
  }, [theme]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      fontFamily: '"SF Mono", ui-monospace, Menlo, monospace',
      fontSize: 16,
      theme: themeRef.current
        ? themeToXterm(themeRef.current)
        : DEFAULT_XTERM_THEME,
      cursorBlink: true,
      // Fallback scrollback for panes that don't capture the mouse (plain
      // shell, Claude's classic renderer). Claude's fullscreen renderer
      // requests SGR mouse tracking, so there the wheel is forwarded to the
      // app and Claude scrolls its own transcript instead.
      scrollback: 50000,
      // With mouse tracking active, xterm.js hands drags to the app rather
      // than selecting. Option+drag forces a native xterm selection —
      // matches iTerm2, and gives an escape hatch when Claude owns the mouse.
      macOptionClickForcesSelection: true,
      allowProposedApi: true,
      // OSC 8 hyperlinks (Claude Code emits these for PR URLs etc.) default
      // to `window.open`, which Tauri's WKWebView blocks and routes through
      // dialog.confirm. Route through plugin-opener instead.
      linkHandler: {
        activate: (_ev, uri) => {
          openUrl(uri).catch((e) => console.error("openUrl failed:", e));
        },
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new ClipboardAddon());
    // Ctrl/Cmd+Click a URL → default browser, again via plugin-opener so
    // WKWebView doesn't intercept the navigation.
    term.loadAddon(
      new WebLinksAddon((event, uri) => {
        event.preventDefault();
        openUrl(uri).catch((e) => console.error("openUrl failed:", e));
      }),
    );
    // xterm's default DOM renderer: @xterm/addon-canvas reaches into v5
    // internals that v6 removed (`_linkifier2`), and WebGL + WKWebView has
    // known issues on macOS. DOM is plenty fast for interactive shells.
    term.open(container);
    fit.fit();
    term.focus();
    termRef.current = term;

    const teardownExtras = onReadyRef.current?.(term, container);

    const dataSub = term.onData((data) => {
      const bytes = Array.from(new TextEncoder().encode(data));
      api.sendInput(sessionId, bytes).catch((e) => {
        console.error("send_input failed:", e);
      });
    });
    const resizeSub = term.onResize(({ cols, rows }) => {
      api.resizeSession(sessionId, cols, rows).catch((e) => {
        console.error("resize_session failed:", e);
      });
    });

    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (chunk) => {
      term.write(new Uint8Array(chunk));
    };

    let cancelled = false;
    api
      .attachSession(sessionId, channel)
      .then((scrollback) => {
        if (cancelled) return;
        if (scrollback.length > 0) {
          term.write(new Uint8Array(scrollback));
        }
        // Final resize so the backend matches xterm's cols/rows right away.
        api.resizeSession(sessionId, term.cols, term.rows).catch(() => {});
      })
      .catch((e) => {
        term.write(`\r\n\x1b[31m[attach failed: ${String(e)}]\x1b[0m\r\n`);
      });

    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // xterm throws if the container has zero size (e.g. mid-transition).
      }
    });
    ro.observe(container);

    return () => {
      cancelled = true;
      ro.disconnect();
      dataSub.dispose();
      resizeSub.dispose();
      teardownExtras?.();
      term.dispose();
      termRef.current = null;
      // Stop the backend fanning bytes to this dead pane. Its retain-on-error
      // path never fires on its own: the channel keeps succeeding as long as
      // its JS callback is registered, so we must detach explicitly.
      api.detachSession(sessionId, channel.id).catch(() => {});
      // Sever the channel's reference to `term`. Tauri registers the
      // channel's callback in a global registry and only unregisters it on an
      // end-of-stream message, which command-arg channels never send — so the
      // closure would otherwise pin the whole disposed terminal (and its
      // 50k-line scrollback) in the webview forever.
      channel.onmessage = () => {};
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  return { containerRef, termRef };
}
