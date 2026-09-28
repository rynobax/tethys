import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  DndContext,
  DragOverlay,
  PointerSensor,
  closestCenter,
  pointerWithin,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type CollisionDetection,
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

import type { Folder, FolderId, Workspace, WorkspaceId } from "./types";
import { GithubChip } from "./GithubChip";
import {
  blockerCandidates,
  folderSections,
  linkPrs,
  type WorkspaceRow as TreeRow,
} from "./workspaceDerived";

type Props = {
  workspaces: Workspace[];
  folders: Folder[];
  selectedId: WorkspaceId | null;
  onSelect: (id: WorkspaceId) => void;
  onReorder: (ids: WorkspaceId[]) => void;
  onMoveToFolder: (
    ids: WorkspaceId[],
    folder: FolderId | null,
    order: WorkspaceId[],
  ) => void;
  onReorderFolders: (ids: FolderId[]) => void;
  onCreateFolder: (name: string) => void;
  onRenameFolder: (id: FolderId, name: string) => void;
  onDeleteFolder: (id: FolderId) => void;
  onSetFolderCollapsed: (id: FolderId, collapsed: boolean) => void;
  onDelete: (ws: Workspace) => void;
  onClearTurn: (ws: Workspace) => void;
  onSetBlocker: (ws: Workspace, blockerId: WorkspaceId | null) => void;
  workspaceNeedsTurn: (ws: Workspace) => boolean;
  workspaceWorking: (ws: Workspace) => boolean;
};

const HEADER_PREFIX = "folder:";
const DEFAULT_HEADER = `${HEADER_PREFIX}default`;
const headerId = (folder: FolderId | null) =>
  folder === null ? DEFAULT_HEADER : `${HEADER_PREFIX}${folder}`;
const isHeaderId = (id: string) => id.startsWith(HEADER_PREFIX);
const folderFromHeaderId = (id: string): FolderId | null =>
  id === DEFAULT_HEADER ? null : id.slice(HEADER_PREFIX.length);

// A blocker stack moves as one, so a drag can't split it across folders where
// the nesting could no longer be drawn.
type Block = { rows: TreeRow[]; ids: WorkspaceId[] };

type Section = { folder: Folder | null; blocks: Block[] };

function cutIntoBlocks(rows: TreeRow[]): Block[] {
  const out: Block[] = [];
  for (const row of rows) {
    if (row.depth === 0 || out.length === 0) {
      out.push({ rows: [row], ids: [row.workspace.id] });
    } else {
      const last = out[out.length - 1];
      last.rows.push(row);
      last.ids.push(row.workspace.id);
    }
  }
  return out;
}

type PlacedBlock = { block: Block; folder: FolderId | null };

// A header drop means "append", so the pointer must really be over one; else a
// row dragged up would resolve to its header and land at the bottom.
const collisionDetection: CollisionDetection = (args) => {
  const under = pointerWithin(args);
  return under.length > 0 ? under : closestCenter(args);
};

type ActiveDrag =
  | { type: "workspace"; id: WorkspaceId }
  | { type: "folder"; id: FolderId };

export function Sidebar({
  workspaces,
  folders,
  selectedId,
  onSelect,
  onReorder,
  onMoveToFolder,
  onReorderFolders,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
  onSetFolderCollapsed,
  onDelete,
  onClearTurn,
  onSetBlocker,
  workspaceNeedsTurn,
  workspaceWorking,
}: Props) {
  const sections: Section[] = useMemo(
    () =>
      folderSections(workspaces, folders).map((s) => ({
        folder: s.folder,
        blocks: cutIntoBlocks(s.rows),
      })),
    [workspaces, folders],
  );

  const rootOf = useMemo(() => {
    const map = new Map<WorkspaceId, WorkspaceId>();
    for (const section of sections) {
      for (const block of section.blocks) {
        for (const id of block.ids) map.set(id, block.ids[0]);
      }
    }
    return map;
  }, [sections]);

  const placed: PlacedBlock[] = useMemo(
    () =>
      sections.flatMap((s) =>
        s.blocks.map((block) => ({ block, folder: s.folder?.id ?? null })),
      ),
    [sections],
  );

  const showHeaders = folders.length > 0;

  const [menu, setMenu] = useState<{
    ws: Workspace;
    x: number;
    y: number;
  } | null>(null);
  const [folderMenu, setFolderMenu] = useState<{
    folder: Folder;
    x: number;
    y: number;
  } | null>(null);
  const [renaming, setRenaming] = useState<FolderId | null>(null);
  const [activeDrag, setActiveDrag] = useState<ActiveDrag | null>(null);

  const sensors = useSensors(
    // No KeyboardSensor: Enter on a focused row would trap you in drag mode.
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  useEffect(() => {
    if (!activeDrag) return;
    const stillThere =
      activeDrag.type === "workspace"
        ? workspaces.some((w) => w.id === activeDrag.id)
        : folders.some((f) => f.id === activeDrag.id);
    if (!stillThere) setActiveDrag(null);
  }, [activeDrag, workspaces, folders]);

  useEffect(() => {
    if (!activeDrag) return;
    const clear = () => setActiveDrag(null);
    window.addEventListener("blur", clear);
    return () => window.removeEventListener("blur", clear);
  }, [activeDrag]);

  const handleDragStart = (event: DragStartEvent) => {
    const id = String(event.active.id);
    if (event.active.data.current?.type === "folder") {
      setActiveDrag({ type: "folder", id: folderFromHeaderId(id) ?? id });
    } else {
      setActiveDrag({ type: "workspace", id });
    }
  };

  const folderAtTarget = (overId: string): FolderId | null | undefined => {
    if (isHeaderId(overId)) return folderFromHeaderId(overId);
    const hit = placed.find((p) => p.block.ids.includes(overId));
    return hit ? hit.folder : undefined;
  };

  const dropFolder = (draggedFolder: FolderId, overId: string) => {
    const target = folderAtTarget(overId);
    // Default (`null`) is always first.
    if (target === undefined || target === null || target === draggedFolder) {
      return;
    }
    const ids = folders.map((f) => f.id);
    const from = ids.indexOf(draggedFolder);
    const to = ids.indexOf(target);
    if (from < 0 || to < 0) return;
    onReorderFolders(arrayMove(ids, from, to));
  };

  const dropWorkspace = (draggedId: WorkspaceId, overId: string) => {
    const root = rootOf.get(draggedId);
    if (!root) return;
    const from = placed.findIndex((p) => p.block.ids[0] === root);
    if (from < 0) return;
    const destination = folderAtTarget(overId);
    if (destination === undefined) return;

    // Indexed against the list before the block is lifted out, as `arrayMove`
    // does, so a downward drop lands where the preview showed it.
    let to: number;
    if (isHeaderId(overId)) {
      to =
        placed.reduce((acc, p, i) => (p.folder === destination ? i : acc), -1) +
        1;
    } else {
      to = placed.findIndex((p) => p.block.ids.includes(overId));
      if (to < 0 || to === from) return;
    }

    const next = [...placed];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, { block: moved.block, folder: destination });

    const order = next.flatMap((p) => p.block.ids);
    if (moved.folder === destination) onReorder(order);
    else onMoveToFolder(moved.block.ids, destination, order);
  };

  const handleDragEnd = (event: DragEndEvent) => {
    setActiveDrag(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const activeId = String(active.id);
    const overId = String(over.id);
    if (active.data.current?.type === "folder") {
      dropFolder(folderFromHeaderId(activeId) ?? activeId, overId);
    } else {
      dropWorkspace(activeId, overId);
    }
  };

  const dragged =
    activeDrag?.type === "workspace"
      ? workspaces.find((w) => w.id === activeDrag.id) ?? null
      : null;
  const draggedFolder =
    activeDrag?.type === "folder"
      ? folders.find((f) => f.id === activeDrag.id) ?? null
      : null;

  const rowProps = (w: Workspace) => ({
    workspace: w,
    selected: w.id === selectedId,
    needsTurn: workspaceNeedsTurn(w),
    working: workspaceWorking(w),
  });

  return (
    <>
      <ul className="workspace-list">
        {workspaces.length === 0 && (
          <li className="empty">No workspaces yet.</li>
        )}
        <DndContext
          sensors={sensors}
          collisionDetection={collisionDetection}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
          onDragCancel={() => setActiveDrag(null)}
        >
          <SortableContext
            items={folders.map((f) => headerId(f.id))}
            strategy={verticalListSortingStrategy}
          >
            {sections.map(({ folder, blocks }) => {
              const rows = blocks.flatMap((b) => b.rows);
              const collapsed = folder?.collapsed ?? false;
              return (
                <Fragment key={headerId(folder?.id ?? null)}>
                  {showHeaders &&
                    (folder ? (
                      <SortableFolderHeader
                        folder={folder}
                        count={rows.length}
                        renaming={renaming === folder.id}
                        onToggle={() =>
                          onSetFolderCollapsed(folder.id, !folder.collapsed)
                        }
                        onRename={(name) => {
                          setRenaming(null);
                          if (name && name !== folder.name) {
                            onRenameFolder(folder.id, name);
                          }
                        }}
                        onContextMenu={(x, y) => setFolderMenu({ folder, x, y })}
                      />
                    ) : (
                      <DefaultFolderHeader count={rows.length} />
                    ))}
                  {!collapsed && (
                    <SortableContext
                      items={blocks.map((b) => b.ids[0])}
                      strategy={verticalListSortingStrategy}
                    >
                      {rows.map(({ workspace: w, depth }) =>
                        depth === 0 ? (
                          <SortableWorkspaceRow
                            key={w.id}
                            {...rowProps(w)}
                            onSelect={() => onSelect(w.id)}
                            onContextMenu={(x, y) => setMenu({ ws: w, x, y })}
                          />
                        ) : (
                          <DraggableWorkspaceRow
                            key={w.id}
                            {...rowProps(w)}
                            depth={depth}
                            onSelect={() => onSelect(w.id)}
                            onContextMenu={(x, y) => setMenu({ ws: w, x, y })}
                          />
                        ),
                      )}
                    </SortableContext>
                  )}
                </Fragment>
              );
            })}
          </SortableContext>
          <DragOverlay>
            {dragged ? (
              <WorkspaceRow
                {...rowProps(dragged)}
                isDragging
                onSelect={() => {}}
                onContextMenu={() => {}}
              />
            ) : draggedFolder ? (
              <li className="folder-header is-dragging">
                <span className="disclosure">▸</span>
                {draggedFolder.name}
              </li>
            ) : null}
          </DragOverlay>
        </DndContext>
      </ul>
      <NewFolderRow onCreate={onCreateFolder} />
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          workspace={menu.ws}
          hasTurn={workspaceNeedsTurn(menu.ws)}
          blockerOptions={blockerCandidates(workspaces, menu.ws.id)}
          onClose={() => setMenu(null)}
          onDelete={onDelete}
          onClearTurn={onClearTurn}
          onSetBlocker={onSetBlocker}
        />
      )}
      {folderMenu && (
        <FolderContextMenu
          x={folderMenu.x}
          y={folderMenu.y}
          onClose={() => setFolderMenu(null)}
          onRename={() => setRenaming(folderMenu.folder.id)}
          onDelete={() => onDeleteFolder(folderMenu.folder.id)}
        />
      )}
    </>
  );
}

// Droppable, not sortable: Default accepts rows but has no position to trade.
function DefaultFolderHeader({ count }: { count: number }) {
  const { setNodeRef, isOver } = useDroppable({ id: DEFAULT_HEADER });
  return (
    <li
      ref={setNodeRef}
      className={`folder-header is-default${isOver ? " is-over" : ""}`}
    >
      <span className="folder-name">Default</span>
      <span className="folder-count">{count}</span>
    </li>
  );
}

function SortableFolderHeader({
  folder,
  count,
  renaming,
  onToggle,
  onRename,
  onContextMenu,
}: {
  folder: Folder;
  count: number;
  renaming: boolean;
  onToggle: () => void;
  onRename: (name: string) => void;
  onContextMenu: (x: number, y: number) => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
    isOver,
  } = useSortable({ id: headerId(folder.id), data: { type: "folder" } });

  const classes = [
    "folder-header",
    isDragging ? "is-dragging" : "",
    isOver ? "is-over" : "",
    folder.collapsed ? "is-collapsed" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <li
      ref={setNodeRef}
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0 : undefined,
      }}
      {...attributes}
      // Dragging by the header would fight the rename field for the pointer.
      {...(renaming ? {} : listeners)}
      className={classes}
      onClick={renaming ? undefined : onToggle}
      onContextMenu={(e) => {
        e.preventDefault();
        onContextMenu(e.clientX, e.clientY);
      }}
    >
      <span className={`disclosure${folder.collapsed ? "" : " open"}`}>▸</span>
      {renaming ? (
        <FolderNameInput initial={folder.name} onCommit={onRename} />
      ) : (
        <span className="folder-name">{folder.name}</span>
      )}
      <span className="folder-count">{count}</span>
    </li>
  );
}

function FolderNameInput({
  initial,
  onCommit,
  onCancel,
}: {
  initial: string;
  onCommit: (name: string) => void;
  onCancel?: () => void;
}) {
  const [value, setValue] = useState(initial);
  const done = useRef(false);
  const commit = () => {
    if (done.current) return;
    done.current = true;
    onCommit(value.trim());
  };
  return (
    <input
      className="folder-name-input"
      autoFocus
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
      onBlur={commit}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") commit();
        if (e.key === "Escape") {
          done.current = true;
          (onCancel ?? (() => onCommit("")))();
        }
      }}
    />
  );
}

function NewFolderRow({ onCreate }: { onCreate: (name: string) => void }) {
  const [naming, setNaming] = useState(false);
  return (
    <div className="sidebar-newfolder">
      {naming ? (
        <FolderNameInput
          initial="New folder"
          onCommit={(name) => {
            setNaming(false);
            if (name) onCreate(name);
          }}
          onCancel={() => setNaming(false)}
        />
      ) : (
        <button type="button" onClick={() => setNaming(true)}>
          <span className="new-folder-plus" aria-hidden="true">
            +
          </span>
          New folder
        </button>
      )}
    </div>
  );
}

function SortableWorkspaceRow({
  workspace,
  selected,
  needsTurn,
  working,
  onSelect,
  onContextMenu,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  working: boolean;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: workspace.id, data: { type: "workspace" } });

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

// Not sortable: a blocked row's drag resolves to its blocker's stack.
function DraggableWorkspaceRow({
  workspace,
  selected,
  needsTurn,
  working,
  depth,
  onSelect,
  onContextMenu,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  working: boolean;
  depth: number;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, isDragging } =
    useDraggable({ id: workspace.id, data: { type: "workspace" } });

  return (
    <WorkspaceRow
      workspace={workspace}
      selected={selected}
      needsTurn={needsTurn}
      working={working}
      isDragging={isDragging}
      depth={depth}
      onSelect={onSelect}
      onContextMenu={onContextMenu}
      dndProps={{
        ref: setNodeRef,
        style: {
          transform: CSS.Translate.toString(transform),
          opacity: isDragging ? 0 : undefined,
        },
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
  isDragging?: boolean;
  depth?: number;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
  dndProps?: DndProps;
}) {
  const status = workspace.status.kind;
  const statusEdge =
    status === "ready"
      ? needsTurn
        ? "status-turn"
        : working
          ? "status-working"
          : ""
      : "";
  const classes = [
    selected ? "selected" : "",
    depth > 0 ? "is-blocked" : "",
    isDragging ? "is-dragging" : "",
    status === "creating" || status === "queued" ? "pending" : "",
    status === "creation_failed" ? "creation-failed" : "",
    statusEdge,
  ]
    .filter(Boolean)
    .join(" ");

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
      {status === "creating" && <div className="pending-label">creating…</div>}
      {status === "queued" && (
        <div className="pending-label" title="Tethys sets up one workspace at a time">
          queued…
        </div>
      )}
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

function useDismissOnOutsideClick(onClose: () => void) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const handle = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", handle);
    return () => document.removeEventListener("mousedown", handle);
  }, [onClose]);
  return ref;
}

function menuPosition(x: number, y: number, height: number) {
  const ESTIMATED_W = 180;
  return {
    left: Math.min(x, window.innerWidth - ESTIMATED_W - 4),
    top: Math.min(y, window.innerHeight - height - 4),
  };
}

function FolderContextMenu({
  x,
  y,
  onClose,
  onRename,
  onDelete,
}: {
  x: number;
  y: number;
  onClose: () => void;
  onRename: () => void;
  onDelete: () => void;
}) {
  const ref = useDismissOnOutsideClick(onClose);
  const wrap = (fn: () => void) => () => {
    fn();
    onClose();
  };
  return (
    <div
      ref={ref}
      className="context-menu"
      style={menuPosition(x, y, 70)}
      role="menu"
    >
      <button type="button" role="menuitem" onClick={wrap(onRename)}>
        Rename
      </button>
      <button
        type="button"
        role="menuitem"
        className="danger"
        onClick={wrap(onDelete)}
      >
        Delete folder
      </button>
    </div>
  );
}

function ContextMenu({
  x,
  y,
  workspace,
  hasTurn,
  blockerOptions,
  onClose,
  onDelete,
  onClearTurn,
  onSetBlocker,
}: {
  x: number;
  y: number;
  workspace: Workspace;
  hasTurn: boolean;
  blockerOptions: Workspace[];
  onClose: () => void;
  onDelete: (ws: Workspace) => void;
  onClearTurn: (ws: Workspace) => void;
  onSetBlocker: (ws: Workspace, blockerId: WorkspaceId | null) => void;
}) {
  const ref = useDismissOnOutsideClick(onClose);
  const isReady = workspace.status.kind === "ready";
  const [pickingBlocker, setPickingBlocker] = useState(false);

  const wrap = (fn: () => void) => () => {
    fn();
    onClose();
  };

  return (
    <div
      ref={ref}
      className="context-menu"
      style={menuPosition(x, y, 110)}
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
      {blockerOptions.length > 0 && (
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
