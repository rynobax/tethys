import { useEffect, useMemo, useRef, useState } from "react";
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
  type DraggableAttributes,
} from "@dnd-kit/core";
import type { SyntheticListenerMap } from "@dnd-kit/core/dist/hooks/utilities";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import type { MemorySnapshot, Workspace, WorkspaceId } from "./types";
import { GithubChip } from "./GithubChip";
import { SidebarWorkspaceMemoryChip } from "./DevServerControls";

type Props = {
  /** Workspaces that should appear in the sidebar (soft-deleted already filtered out). */
  workspaces: Workspace[];
  selectedId: WorkspaceId | null;
  onSelect: (id: WorkspaceId) => void;
  onReorder: (ids: WorkspaceId[]) => void;
  onArchiveToggle: (ws: Workspace) => void;
  onDelete: (ws: Workspace) => void;
  onClearTurn: (ws: Workspace) => void;
  workspaceNeedsTurn: (ws: Workspace) => boolean;
  /** Latest poller snapshot. Used to render per-row RAM chips for
   *  workspaces with dev servers running. `null` until the first
   *  snapshot lands. */
  memory: MemorySnapshot | null;
};

export function Sidebar({
  workspaces,
  selectedId,
  onSelect,
  onReorder,
  onArchiveToggle,
  onDelete,
  onClearTurn,
  workspaceNeedsTurn,
  memory,
}: Props) {
  const { active, archived } = useMemo(() => {
    const active: Workspace[] = [];
    const archived: Workspace[] = [];
    for (const w of workspaces) {
      if (w.archived_at) archived.push(w);
      else active.push(w);
    }
    archived.sort((a, b) =>
      (b.archived_at ?? "").localeCompare(a.archived_at ?? ""),
    );
    return { active, archived };
  }, [workspaces]);

  const [archivedExpanded, setArchivedExpanded] = useState(false);
  const [menu, setMenu] = useState<{
    ws: Workspace;
    x: number;
    y: number;
  } | null>(null);
  const [activeId, setActiveId] = useState<WorkspaceId | null>(null);

  const sensors = useSensors(
    // 5px activation distance prevents a single click from being interpreted
    // as a drag start, which would swallow row selection.
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  // Defensive: clear stuck drag state if the dragged workspace falls out of
  // the list (archived/deleted mid-drag), or if the window loses focus.
  useEffect(() => {
    if (activeId && !active.some((w) => w.id === activeId)) {
      setActiveId(null);
    }
  }, [activeId, active]);

  useEffect(() => {
    if (!activeId) return;
    const clear = () => setActiveId(null);
    window.addEventListener("blur", clear);
    return () => window.removeEventListener("blur", clear);
  }, [activeId]);

  const handleDragStart = (event: DragStartEvent) => {
    setActiveId(event.active.id as WorkspaceId);
  };

  const handleDragEnd = (event: DragEndEvent) => {
    setActiveId(null);
    const { active: dragged, over } = event;
    if (!over || dragged.id === over.id) return;
    const from = active.findIndex((w) => w.id === dragged.id);
    const to = active.findIndex((w) => w.id === over.id);
    if (from < 0 || to < 0) return;
    onReorder(arrayMove(active, from, to).map((w) => w.id));
  };

  const activeWorkspace = activeId
    ? active.find((w) => w.id === activeId) ?? null
    : null;

  return (
    <>
      <ul className="workspace-list">
        {active.length === 0 && archived.length === 0 && (
          <li className="empty">No workspaces yet.</li>
        )}
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
          onDragCancel={() => setActiveId(null)}
        >
          <SortableContext
            items={active.map((w) => w.id)}
            strategy={verticalListSortingStrategy}
          >
            {active.map((w) => (
              <SortableWorkspaceRow
                key={w.id}
                workspace={w}
                selected={w.id === selectedId}
                needsTurn={workspaceNeedsTurn(w)}
                memory={memory}
                onSelect={() => onSelect(w.id)}
                onContextMenu={(x, y) => setMenu({ ws: w, x, y })}
              />
            ))}
          </SortableContext>
          <DragOverlay>
            {activeWorkspace ? (
              <WorkspaceRow
                workspace={activeWorkspace}
                selected={activeWorkspace.id === selectedId}
                needsTurn={workspaceNeedsTurn(activeWorkspace)}
                memory={memory}
                isDragging
                onSelect={() => {}}
                onContextMenu={() => {}}
              />
            ) : null}
          </DragOverlay>
        </DndContext>

        {archived.length > 0 && (
          <li
            className="archive-header"
            onClick={() => setArchivedExpanded((v) => !v)}
          >
            <span className={`disclosure${archivedExpanded ? " open" : ""}`}>
              ▸
            </span>
            Archived
            <span className="archive-count">{archived.length}</span>
          </li>
        )}
        {archivedExpanded &&
          archived.map((w) => (
            <WorkspaceRow
              key={w.id}
              workspace={w}
              selected={w.id === selectedId}
              needsTurn={workspaceNeedsTurn(w)}
              memory={memory}
              isArchived
              onSelect={() => onSelect(w.id)}
              onContextMenu={(x, y) => setMenu({ ws: w, x, y })}
            />
          ))}
      </ul>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          workspace={menu.ws}
          hasTurn={workspaceNeedsTurn(menu.ws)}
          onClose={() => setMenu(null)}
          onArchiveToggle={onArchiveToggle}
          onDelete={onDelete}
          onClearTurn={onClearTurn}
        />
      )}
    </>
  );
}

function SortableWorkspaceRow({
  workspace,
  selected,
  needsTurn,
  memory,
  onSelect,
  onContextMenu,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  memory: MemorySnapshot | null;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: workspace.id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0 : undefined,
  };

  return (
    <WorkspaceRow
      workspace={workspace}
      selected={selected}
      needsTurn={needsTurn}
      memory={memory}
      isDragging={isDragging}
      onSelect={onSelect}
      onContextMenu={onContextMenu}
      dndProps={{
        ref: setNodeRef,
        style,
        attributes,
        listeners,
      }}
    />
  );
}

type DndProps = {
  ref: (node: HTMLElement | null) => void;
  style: React.CSSProperties;
  attributes: DraggableAttributes;
  listeners: SyntheticListenerMap | undefined;
};

function WorkspaceRow({
  workspace,
  selected,
  needsTurn,
  memory,
  isArchived = false,
  isDragging = false,
  onSelect,
  onContextMenu,
  dndProps,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  memory: MemorySnapshot | null;
  isArchived?: boolean;
  isDragging?: boolean;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
  dndProps?: DndProps;
}) {
  const status = workspace.status.kind;
  const classes = [
    selected ? "selected" : "",
    isArchived ? "is-archived" : "",
    isDragging ? "is-dragging" : "",
    status === "creating" ? "pending" : "",
    status === "creation_failed" ? "creation-failed" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <li
      ref={dndProps?.ref}
      style={dndProps?.style}
      {...(dndProps?.attributes ?? {})}
      {...(dndProps?.listeners ?? {})}
      className={classes}
      onClick={onSelect}
      onContextMenu={(e) => {
        e.preventDefault();
        onContextMenu(e.clientX, e.clientY);
      }}
    >
      <div className="workspace-name">
        {status === "creating" && <Spinner />}
        <span className="workspace-name-text" title={workspace.branch}>
          {workspace.branch}
        </span>
        {status === "ready" && needsTurn && (
          <span
            className="turn-dot"
            title="Your turn"
            aria-label="your turn"
          />
        )}
      </div>
      {status === "creating" && <div className="pending-label">creating…</div>}
      {status === "creation_failed" && (
        <div className="pending-label">creation failed</div>
      )}
      {status === "ready" && workspace.repo_links.length > 0 && (
        <ul className="workspace-repo-list">
          {workspace.repo_links.map((r) => (
            <li key={r.repo_key}>
              <span className="repo-key">{r.repo_key}</span>
              {r.github && (
                <div className="repo-gh-footer">
                  <GithubChip status={r.github} linkable={false} />
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
      {status === "ready" && (
        <SidebarWorkspaceMemoryChip workspace={workspace} memory={memory} />
      )}
    </li>
  );
}

function ContextMenu({
  x,
  y,
  workspace,
  hasTurn,
  onClose,
  onArchiveToggle,
  onDelete,
  onClearTurn,
}: {
  x: number;
  y: number;
  workspace: Workspace;
  hasTurn: boolean;
  onClose: () => void;
  onArchiveToggle: (ws: Workspace) => void;
  onDelete: (ws: Workspace) => void;
  onClearTurn: (ws: Workspace) => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const isReady = workspace.status.kind === "ready";

  useEffect(() => {
    const handle = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", handle);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", handle);
      document.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  // Keep the menu inside the viewport.
  const viewportW = window.innerWidth;
  const viewportH = window.innerHeight;
  const ESTIMATED_W = 180;
  const ESTIMATED_H = 110;
  const left = Math.min(x, viewportW - ESTIMATED_W - 4);
  const top = Math.min(y, viewportH - ESTIMATED_H - 4);

  const wrap = (fn: () => void) => () => {
    fn();
    onClose();
  };

  return (
    <div
      ref={ref}
      className="context-menu"
      style={{ left, top }}
      role="menu"
    >
      {hasTurn && (
        <button
          type="button"
          role="menuitem"
          onClick={wrap(() => onClearTurn(workspace))}
        >
          Clear notification
        </button>
      )}
      {isReady && (
        <button
          type="button"
          role="menuitem"
          onClick={wrap(() => onArchiveToggle(workspace))}
        >
          {workspace.archived_at ? "Unarchive" : "Archive"}
        </button>
      )}
      {isReady && <div className="context-menu-sep" />}
      <button
        type="button"
        role="menuitem"
        className="danger"
        onClick={wrap(() => onDelete(workspace))}
      >
        {workspace.status.kind === "creation_failed" ? "Dismiss" : "Delete"}
      </button>
    </div>
  );
}

function Spinner() {
  return <span className="spinner" aria-hidden="true" />;
}
