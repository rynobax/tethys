import { scriptEndpoint, usePtyTerminal } from "./usePtyTerminal";

interface Props {
  scriptId: string;
}

/**
 * xterm.js surface for a script's PTY.
 *
 * Everything a terminal pane does lives in `usePtyTerminal`; scripts add
 * nothing to it. The Claude-specific paste, keybinding and drag-drop handlers
 * belong to `SessionTerminal` — script logs don't need the `[Image #N]` flow.
 */
export function ScriptTerminal({ scriptId }: Props) {
  const { containerRef } = usePtyTerminal(scriptId, scriptEndpoint);
  return <div className="session-terminal" ref={containerRef} />;
}
