import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ClipboardAddon } from "@xterm/addon-clipboard";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";

import * as api from "./ipc/commands";
import { Channel, openUrl } from "./ipc/commands";
import { themeToXterm, useTheme } from "./theme";

const DEFAULT_XTERM_THEME = {
  background: "#0a0a0a",
  foreground: "#e8e8e8",
};

/** `onReady`'s teardown runs before dispose. `onReady` needn't be stable. */
export function usePtyTerminal(
  sessionId: string,
  onReady: (term: Terminal, container: HTMLDivElement) => () => void,
) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const theme = useTheme();
  // Read through a ref so a theme change doesn't rebuild xterm.
  const themeRef = useRef(theme);
  themeRef.current = theme;
  const onReadyRef = useRef(onReady);
  onReadyRef.current = onReady;

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
      scrollback: 50000,
      // An escape hatch to select text while the app owns the mouse.
      macOptionClickForcesSelection: true,
      allowProposedApi: true,
      // The default opens a blank window and navigates it afterwards, so main's
      // window-open handler never sees the URL.
      linkHandler: {
        activate: (_ev, uri) => {
          openUrl(uri).catch((e) => console.error("openUrl failed:", e));
        },
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new ClipboardAddon());
    term.loadAddon(
      new WebLinksAddon((event, uri) => {
        event.preventDefault();
        openUrl(uri).catch((e) => console.error("openUrl failed:", e));
      }),
    );
    term.open(container);
    fit.fit();
    term.focus();
    termRef.current = term;

    const teardownExtras = onReadyRef.current(term, container);

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

    // Scrollback comes first on the channel, so chunks are written in order.
    const channel = new Channel<Uint8Array>();
    channel.onmessage = (chunk) => term.write(chunk);

    let cancelled = false;
    api
      .attachSession(sessionId, channel)
      .then(() => {
        if (cancelled) return;
        api.resizeSession(sessionId, term.cols, term.rows).catch(() => {});
      })
      .catch((e) => {
        term.write(`\r\n\x1b[31m[attach failed: ${String(e)}]\x1b[0m\r\n`);
      });

    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // Throws on a zero-size container.
      }
    });
    ro.observe(container);

    return () => {
      cancelled = true;
      ro.disconnect();
      dataSub.dispose();
      resizeSub.dispose();
      teardownExtras();
      term.dispose();
      termRef.current = null;
      // The daemon never drops a live channel on its own.
      api.detachSession(sessionId, channel.id).catch(() => {});
      channel.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  return containerRef;
}
