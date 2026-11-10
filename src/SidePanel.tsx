import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "./ipc/commands";
import type { Workspace } from "./types";

const WIDTHS_KEY = "tethys.sidePanel.widths";
const COLLAPSED_KEY = "tethys.sidePanel.collapsedByWorkspace";
const DEFAULT_WIDTH = 360;
const MIN_WIDTH = 280;

function loadMap<T>(
  key: string,
  valid: (v: unknown) => v is T,
): Record<string, T> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "");
    if (parsed && typeof parsed === "object") {
      return Object.fromEntries(
        Object.entries(parsed as Record<string, unknown>).filter(
          (e): e is [string, T] => valid(e[1]),
        ),
      );
    }
  } catch {
    /* unparseable starts fresh */
  }
  return {};
}

const isPanelWidth = (v: unknown): v is number =>
  typeof v === "number" && v >= MIN_WIDTH;
const isBoolean = (v: unknown): v is boolean => typeof v === "boolean";

interface Props {
  workspace: Workspace;
  notes: string;
  onNotesChange: (notes: string) => void;
}

export function SidePanel({
  workspace,
  notes,
  onNotesChange,
}: Props) {
  const [collapsedMap, setCollapsedMap] = useState(() =>
    loadMap(COLLAPSED_KEY, isBoolean),
  );
  const collapsed = collapsedMap[workspace.id] ?? true;
  const persistCollapsed = (value: boolean) => {
    setCollapsedMap((prev) => {
      const next = { ...prev, [workspace.id]: value };
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next));
      return next;
    });
  };
  const [widths, setWidths] = useState(() => loadMap(WIDTHS_KEY, isPanelWidth));
  const width = widths[workspace.id] ?? DEFAULT_WIDTH;
  const setWorkspaceWidth = (w: number) => {
    setWidths((prev) => {
      const next = { ...prev, [workspace.id]: w };
      localStorage.setItem(WIDTHS_KEY, JSON.stringify(next));
      return next;
    });
  };

  if (collapsed) {
    return (
      <button
        type="button"
        className="side-rail"
        onClick={() => persistCollapsed(false)}
        title="Expand notes"
      >
        <span className="side-rail-label">
          Notes
          {notes.trim() && <span className="side-rail-dot" />}
        </span>
      </button>
    );
  }

  return (
    <aside className="side-panel" style={{ width }}>
      <ResizeHandle onResize={setWorkspaceWidth} />
      <div className="side-tabs">
        <span className="side-tab active">Notes</span>
        <button
          type="button"
          className="side-collapse"
          onClick={() => persistCollapsed(true)}
          title="Collapse notes"
        >
          »
        </button>
      </div>
      <div className="side-body">
        <NotesTab
          key={workspace.id}
          workspaceId={workspace.id}
          notes={notes}
          onNotesChange={onNotesChange}
        />
      </div>
    </aside>
  );
}

// A move with no button held ends the drag, and so does a window blur, in case
// the release is never seen.
function ResizeHandle({ onResize }: { onResize: (width: number) => void }) {
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    const startX = e.clientX;
    const startWidth =
      handle.parentElement?.getBoundingClientRect().width ?? MIN_WIDTH;
    const max = Math.floor(window.innerWidth * 0.7);

    const onMove = (ev: PointerEvent) => {
      if ((ev.buttons & 1) === 0) {
        finish();
        return;
      }
      const next = startWidth + (startX - ev.clientX);
      onResize(Math.max(MIN_WIDTH, Math.min(max, next)));
    };
    const finish = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", finish);
      handle.removeEventListener("pointercancel", finish);
      window.removeEventListener("blur", finish);
      if (handle.hasPointerCapture(e.pointerId)) {
        handle.releasePointerCapture(e.pointerId);
      }
      document.body.style.cursor = "";
    };

    document.body.style.cursor = "col-resize";
    handle.setPointerCapture(e.pointerId);
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", finish);
    handle.addEventListener("pointercancel", finish);
    window.addEventListener("blur", finish);
  };
  return <div className="side-resize" onPointerDown={onPointerDown} />;
}

function NotesTab({
  workspaceId,
  notes,
  onNotesChange,
}: {
  workspaceId: string;
  notes: string;
  onNotesChange: (notes: string) => void;
}) {
  const saveTimer = useRef<number | null>(null);
  const pending = useRef<string | null>(null);

  const save = useCallback(
    (notes: string) => {
      pending.current = null;
      api.setWorkspaceNotes(workspaceId, notes).catch(() => {});
    },
    [workspaceId],
  );

  const flush = useCallback(() => {
    if (saveTimer.current !== null) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    if (pending.current !== null) save(pending.current);
  }, [save]);

  useEffect(() => flush, [flush]);

  const onChange = (value: string) => {
    onNotesChange(value);
    pending.current = value;
    if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = null;
      save(value);
    }, 500);
  };

  return (
    <textarea
      className="notes-textarea"
      value={notes}
      placeholder="Jot down anything about this workspace…"
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
