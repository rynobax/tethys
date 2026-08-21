import { useEffect, useMemo, useRef, useState } from "react";
import {
  DndContext,
  DragOverlay,
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
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import type { Workspace, WorkspaceId } from "./types";
import { GithubChip } from "./GithubChip";
import {
  blockerCandidates,
  linkPrs,
  workspaceTree,
  type WorkspaceRow as TreeRow,
} from "./workspaceDerived";

type Props = {
  /** Workspaces that should appear in the sidebar (soft-deleted already filtered out). */
  workspaces: Workspace[];
  selectedId: WorkspaceId | null;
  onSelect: (id: WorkspaceId) => void;
  onReorder: (ids: WorkspaceId[]) => void;
  onArchiveToggle: (ws: Workspace) => void;
  onDelete: (ws: Workspace) => void;
  onClearTurn: (ws: Workspace) => void;
  /** `blockerId: null` clears the link. */
  onSetBlocker: (ws: Workspace, blockerId: WorkspaceId | null) => void;
  workspaceNeedsTurn: (ws: Workspace) => boolean;
  /** True when a session in the workspace is actively processing (Claude working). */
  workspaceWorking: (ws: Workspace) => boolean;
  /** Names of scripts currently running in the workspace (empty if none). */
  runningScriptNames: (ws: Workspace) => string[];
};

export function Sidebar({
  workspaces,
  selectedId,
  onSelect,
  onReorder,
  onArchiveToggle,
  onDelete,
  onClearTurn,
  onSetBlocker,
  workspaceNeedsTurn,
  workspaceWorking,
  runningScriptNames,
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

  // The order the rows are drawn in, blocked workspaces tucked under their
  // blocker.
  const rows = useMemo(() => workspaceTree(active), [active]);

  // The same rows cut into subtrees — one per top-level row, followed by
  // everything nested beneath it. Dragging moves a whole block, so a blocker
  // takes the workspaces waiting on it along.
  const blocks = useMemo(() => {
    const out: TreeRow[][] = [];
    for (const row of rows) {
      if (row.depth === 0 || out.length === 0) out.push([row]);
      else out[out.length - 1].push(row);
    }
    return out;
  }, [rows]);

  const rootIds = useMemo(
    () => blocks.map((b) => b[0].workspace.id),
    [blocks],
  );

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
    // Pointer-only: no KeyboardSensor, so focusing a row and pressing Enter
    // doesn't trap the user in keyboard drag mode.
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
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
    const from = rootIds.indexOf(dragged.id as WorkspaceId);
    const to = rootIds.indexOf(over.id as WorkspaceId);
    if (from < 0 || to < 0) return;
    // Emitting every id, not just the roots, keeps the stored order identical
    // to the visual one — so a workspace that later stops being blocked stays
    // where it already appeared instead of jumping to the end.
    onReorder(
      arrayMove(blocks, from, to)
        .flat()
        .map((r) => r.workspace.id),
    );
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
            items={rootIds}
            strategy={verticalListSortingStrategy}
          >
            {rows.map(({ workspace: w, depth }) =>
              depth === 0 ? (
                <SortableWorkspaceRow
                  key={w.id}
                  workspace={w}
                  selected={w.id === selectedId}
                  needsTurn={workspaceNeedsTurn(w)}
                  working={workspaceWorking(w)}
                  runningScripts={runningScriptNames(w)}
                  onSelect={() => onSelect(w.id)}
                  onContextMenu={(x, y) => setMenu({ ws: w, x, y })}
                />
              ) : (
                <WorkspaceRow
                  key={w.id}
                  workspace={w}
                  selected={w.id === selectedId}
                  needsTurn={workspaceNeedsTurn(w)}
                  working={workspaceWorking(w)}
                  runningScripts={runningScriptNames(w)}
                  depth={depth}
                  onSelect={() => onSelect(w.id)}
                  onContextMenu={(x, y) => setMenu({ ws: w, x, y })}
                />
              ),
            )}
          </SortableContext>
          <DragOverlay>
            {activeWorkspace ? (
              <WorkspaceRow
                workspace={activeWorkspace}
                selected={activeWorkspace.id === selectedId}
                needsTurn={workspaceNeedsTurn(activeWorkspace)}
                working={workspaceWorking(activeWorkspace)}
                runningScripts={runningScriptNames(activeWorkspace)}
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
              working={workspaceWorking(w)}
              runningScripts={runningScriptNames(w)}
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
          blockerOptions={blockerCandidates(active, menu.ws.id)}
          onClose={() => setMenu(null)}
          onArchiveToggle={onArchiveToggle}
          onDelete={onDelete}
          onClearTurn={onClearTurn}
          onSetBlocker={onSetBlocker}
        />
      )}
    </>
  );
}

function SortableWorkspaceRow({
  workspace,
  selected,
  needsTurn,
  working,
  runningScripts,
  onSelect,
  onContextMenu,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  working: boolean;
  runningScripts: string[];
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
      working={working}
      runningScripts={runningScripts}
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
  working,
  runningScripts,
  isArchived = false,
  isDragging = false,
  depth = 0,
  onSelect,
  onContextMenu,
  dndProps,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  working: boolean;
  runningScripts: string[];
  isArchived?: boolean;
  isDragging?: boolean;
  /** How deep in the blocker tree. 0 is a normal, unblocked row. */
  depth?: number;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
  dndProps?: DndProps;
}) {
  const status = workspace.status.kind;
  // Status tint for live workspaces: yellow when it's your turn, green while a
  // session is working. Your-turn wins over working since it's the actionable
  // state. Idle/cleared rows keep their default background.
  const statusEdge =
    status === "ready" && !isArchived
      ? needsTurn
        ? "status-turn"
        : working
          ? "status-working"
          : ""
      : "";
  const classes = [
    selected ? "selected" : "",
    depth > 0 ? "is-blocked" : "",
    isArchived ? "is-archived" : "",
    isDragging ? "is-dragging" : "",
    status === "creating" ? "pending" : "",
    status === "creation_failed" ? "creation-failed" : "",
    statusEdge,
  ]
    .filter(Boolean)
    .join(" ");

  // One flat list of every PR in the workspace, laid out on a single wrapping
  // row under the name. The repo it came from rides along so a multi-repo
  // workspace can still say which checkout a chip belongs to, on hover.
  const prs = workspace.repo_links.flatMap((r) =>
    linkPrs(r).map((status) => ({ repoKey: r.repo_key, status })),
  );

  return (
    <li
      ref={dndProps?.ref}
      style={
        depth > 0
          ? { ...dndProps?.style, "--depth": depth } as React.CSSProperties
          : dndProps?.style
      }
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
      </div>
      {status === "ready" && runningScripts.length > 0 && (
        <div className="workspace-scripts">
          {runningScripts.map((name) => (
            <span
              key={name}
              className="script-chip"
              title={`Script running: ${name}`}
            >
              {name}
            </span>
          ))}
        </div>
      )}
      {status === "creating" && <div className="pending-label">creating…</div>}
      {status === "creation_failed" && (
        <div className="pending-label">creation failed</div>
      )}
      {status === "ready" && prs.length > 0 && (
        <div className="workspace-prs">
          {prs.map(({ repoKey, status: pr }) => (
            <GithubChip
              key={`${repoKey}#${pr.pr_number}`}
              status={pr}
              context={repoKey}
              linkable={false}
            />
          ))}
        </div>
      )}
    </li>
  );
}


function ContextMenu({
  x,
  y,
  workspace,
  hasTurn,
  blockerOptions,
  onClose,
  onArchiveToggle,
  onDelete,
  onClearTurn,
  onSetBlocker,
}: {
  x: number;
  y: number;
  workspace: Workspace;
  hasTurn: boolean;
  /** Legal blockers for this workspace — cycles already filtered out. */
  blockerOptions: Workspace[];
  onClose: () => void;
  onArchiveToggle: (ws: Workspace) => void;
  onDelete: (ws: Workspace) => void;
  onClearTurn: (ws: Workspace) => void;
  onSetBlocker: (ws: Workspace, blockerId: WorkspaceId | null) => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const isReady = workspace.status.kind === "ready";
  const [pickingBlocker, setPickingBlocker] = useState(false);

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
      {workspace.blocked_by && (
        <button
          type="button"
          role="menuitem"
          onClick={wrap(() => onSetBlocker(workspace, null))}
        >
          Clear blocker
        </button>
      )}
      {/* An archived workspace is already out of the blocking picture — its row
          renders flat in the archived drawer — so offering to give it a blocker
          would set a link with nothing to show for it. Clearing a stale one
          stays available. */}
      {blockerOptions.length > 0 && !workspace.archived_at && (
        <button
          type="button"
          role="menuitem"
          aria-expanded={pickingBlocker}
          onClick={() => setPickingBlocker((v) => !v)}
        >
          <span className={`disclosure${pickingBlocker ? " open" : ""}`}>▸</span>
          Blocked by…
        </button>
      )}
      {pickingBlocker && (
        <div className="context-menu-picker" role="group">
          {blockerOptions.map((candidate) => (
            <button
              key={candidate.id}
              type="button"
              role="menuitem"
              className={
                candidate.id === workspace.blocked_by ? "is-current" : undefined
              }
              title={candidate.branch}
              onClick={wrap(() => onSetBlocker(workspace, candidate.id))}
            >
              {candidate.branch}
            </button>
          ))}
        </div>
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
