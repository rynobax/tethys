import { useEffect, useRef } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
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

interface Props {
  scriptId: string;
}

/**
 * xterm.js surface for a script's PTY. Same shape as `SessionTerminal` but
 * wired to the `*_script` Tauri commands and minus the Claude-specific paste
 * and drag-drop handlers — script logs don't need the `[Image #N]` flow.
 */
export function ScriptTerminal({ scriptId }: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const theme = useTheme();
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
      scrollback: 50000,
      allowProposedApi: true,
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
    term.loadAddon(
      new WebLinksAddon((event, uri) => {
        event.preventDefault();
        openUrl(uri).catch((e) => {
          console.error("openUrl failed:", e);
        });
      }),
    );
    term.open(container);
    fit.fit();
    term.focus();

    termRef.current = term;
    fitRef.current = fit;

    const dataSub = term.onData((data) => {
      const bytes = Array.from(new TextEncoder().encode(data));
      invoke("send_input_script", { scriptId, data: bytes }).catch((e) => {
        console.error("send_input_script failed:", e);
      });
    });
    const resizeSub = term.onResize(({ cols, rows }) => {
      invoke("resize_script", { scriptId, cols, rows }).catch((e) => {
        console.error("resize_script failed:", e);
      });
    });

    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (chunk) => {
      term.write(new Uint8Array(chunk));
    };

    let cancelled = false;
    invoke<number[]>("attach_script", { scriptId, onBytes: channel })
      .then((scrollback) => {
        if (cancelled) return;
        if (scrollback.length > 0) {
          term.write(new Uint8Array(scrollback));
        }
        const { cols, rows } = term;
        invoke("resize_script", { scriptId, cols, rows }).catch(() => {});
      })
      .catch((e) => {
        term.write(`\r\n\x1b[31m[attach failed: ${String(e)}]\x1b[0m\r\n`);
      });

    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // xterm throws if the container has zero size (e.g. during transition)
      }
    });
    ro.observe(container);

    return () => {
      cancelled = true;
      ro.disconnect();
      dataSub.dispose();
      resizeSub.dispose();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      // Stop backend fan-out to this dead pane, then sever the channel's
      // reference to `term` so Tauri's registered callback stops pinning the
      // disposed terminal in the webview. See SessionTerminal for the full
      // rationale.
      invoke("detach_script", { scriptId, channelId: channel.id }).catch(
        () => {},
      );
      channel.onmessage = () => {};
    };
  }, [scriptId]);

  return <div className="session-terminal" ref={containerRef} />;
}
