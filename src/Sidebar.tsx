import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  DndContext,
  DragOverlay,
  PointerSensor,
  closestCenter,
  pointerWithin,
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

import type { Folder, FolderId, PrRef, Workspace, WorkspaceId } from "./types";
import {
  CiIcon,
  GithubChip,
  MergedIcon,
  MergeQueueIcon,
  ReviewIcon,
} from "./GithubChip";
import {
  folderSections,
  linkPrEntries,
  shippablePrs,
  shippingPrs,
  shippingSignal,
  shippingSummary,
  type ShippingSignal,
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
  onStartShipping: (ws: Workspace, prs: PrRef[]) => void;
  onStopShipping: (ws: Workspace) => void;
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

// A parent drags with its children, which can't be dragged on their own.
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
  onStartShipping,
  onStopShipping,
  workspaceNeedsTurn,
  workspaceWorking,
}: Props) {
  // Shipping and Shipped workspaces share their own grid, not the list.
  const listed = useMemo(
    () => workspaces.filter((w) => !w.shipment),
    [workspaces],
  );
  const shipping = useMemo(
    () => workspaces.filter((w) => w.shipment),
    [workspaces],
  );

  const sections: Section[] = useMemo(
    () =>
      folderSections(listed, folders).map((s) => ({
        folder: s.folder,
        blocks: cutIntoBlocks(s.rows),
      })),
    [listed, folders],
  );

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
        ? listed.some((w) => w.id === activeDrag.id)
        : folders.some((f) => f.id === activeDrag.id);
    if (!stillThere) setActiveDrag(null);
  }, [activeDrag, listed, folders]);

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
    const from = placed.findIndex((p) => p.block.ids[0] === draggedId);
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

  // Children that left the list for Shipping, shown on the parent instead.
  const shippingChildren = (w: Workspace) => {
    const kids = workspaces.filter((k) => k.parent === w.id && k.shipment);
    if (kids.length === 0) return undefined;
    return (
      <div className="workspace-shipping-children">
        {kids.map((k) => (
          <ShippingTile
            key={k.id}
            workspace={k}
            mini
            selected={k.id === selectedId}
            working={k.shipment?.kind === "shipping" && workspaceWorking(k)}
            onSelect={() => onSelect(k.id)}
            onContextMenu={(x, y) => setMenu({ ws: k, x, y })}
          />
        ))}
      </div>
    );
  };

  const rowProps = (w: Workspace) => ({
    workspace: w,
    selected: w.id === selectedId,
    needsTurn: workspaceNeedsTurn(w),
    working: workspaceWorking(w),
    shippingChildren: shippingChildren(w),
  });

  return (
    <>
      <ul className="workspace-list">
        {listed.length === 0 && (
          <li className="empty">
            {workspaces.length === 0 ? "No workspaces yet." : "Nothing in progress."}
          </li>
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
                          <WorkspaceRow
                            key={w.id}
                            {...rowProps(w)}
                            isDragging={false}
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
      <ShippingArea
        shipping={shipping}
        selectedId={selectedId}
        workspaceWorking={workspaceWorking}
        onSelect={onSelect}
        onContextMenu={(ws, x, y) => setMenu({ ws, x, y })}
      />
      <NewFolderRow onCreate={onCreateFolder} />
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          workspace={menu.ws}
          hasTurn={workspaceNeedsTurn(menu.ws)}
          liveChildren={
            workspaces.filter((w) => w.parent === menu.ws.id).length
          }
          onClose={() => setMenu(null)}
          onDelete={onDelete}
          onClearTurn={onClearTurn}
          onStartShipping={onStartShipping}
          onStopShipping={onStopShipping}
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
}: {
  initial: string;
  onCommit: (name: string) => void;
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
          onCommit("");
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
  shippingChildren,
  onSelect,
  onContextMenu,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  working: boolean;
  shippingChildren?: React.ReactNode;
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
      shippingChildren={shippingChildren}
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
  shippingChildren,
  isDragging,
  depth = 0,
  onSelect,
  onContextMenu,
  dndProps,
}: {
  workspace: Workspace;
  selected: boolean;
  needsTurn: boolean;
  working: boolean;
  shippingChildren?: React.ReactNode;
  isDragging: boolean;
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
    depth > 0 ? "is-child" : "",
    isDragging ? "is-dragging" : "",
    status === "creating" || status === "queued" ? "pending" : "",
    status === "creation_failed" ? "creation-failed" : "",
    statusEdge,
  ]
    .filter(Boolean)
    .join(" ");

  const prs = workspace.repo_links.flatMap((r) =>
    linkPrEntries(r).map(({ status }) => ({ repoKey: r.repo_key, status })),
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
      {workspace.progress && <Progress {...workspace.progress} />}
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
      {status === "ready" && shippingChildren}
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
  liveChildren,
  onClose,
  onDelete,
  onClearTurn,
  onStartShipping,
  onStopShipping,
}: {
  x: number;
  y: number;
  workspace: Workspace;
  hasTurn: boolean;
  /** Mirrors the backend's refusal, so Delete can say why up front. */
  liveChildren: number;
  onClose: () => void;
  onDelete: (ws: Workspace) => void;
  onClearTurn: (ws: Workspace) => void;
  onStartShipping: (ws: Workspace, prs: PrRef[]) => void;
  onStopShipping: (ws: Workspace) => void;
}) {
  const ref = useDismissOnOutsideClick(onClose);
  const isReady = workspace.status.kind === "ready";
  const candidates = shippablePrs(workspace);
  const [picking, setPicking] = useState(false);

  const wrap = (fn: () => void) => () => {
    fn();
    onClose();
  };

  if (picking) {
    return (
      <div
        ref={ref}
        className="context-menu"
        style={menuPosition(x, y, 60 + candidates.length * 28)}
        role="menu"
      >
        <ShipPicker
          candidates={candidates}
          onShip={(prs) => {
            onStartShipping(workspace, prs);
            onClose();
          }}
        />
      </div>
    );
  }

  return (
    <div
      ref={ref}
      className="context-menu"
      style={menuPosition(x, y, 140)}
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
      {isReady && !workspace.shipment && (
        <button
          type="button"
          role="menuitem"
          disabled={candidates.length === 0}
          title={candidates.length === 0 ? "No open PRs" : undefined}
          onClick={
            candidates.length > 1
              ? () => setPicking(true)
              : wrap(() =>
                  onStartShipping(
                    workspace,
                    candidates.map((c) => ({
                      repo_key: c.repoKey,
                      number: c.status.pr_number,
                    })),
                  ),
                )
          }
        >
          {candidates.length > 1 ? "Ship…" : "Ship"}
        </button>
      )}
      {workspace.shipment && (
        <button
          type="button"
          role="menuitem"
          onClick={wrap(() => onStopShipping(workspace))}
        >
          {workspace.shipment.kind === "shipping" ? "Stop shipping" : "Move back"}
        </button>
      )}
      {isReady && <div className="context-menu-sep" />}
      <button
        type="button"
        role="menuitem"
        className="danger"
        disabled={liveChildren > 0}
        title={
          liveChildren > 0
            ? `Has ${liveChildren} ${liveChildren === 1 ? "child" : "children"}`
            : undefined
        }
        onClick={wrap(() => onDelete(workspace))}
      >
        {workspace.status.kind === "creation_failed" ? "Dismiss" : "Delete"}
      </button>
    </div>
  );
}

function ShipPicker({
  candidates,
  onShip,
}: {
  candidates: ReturnType<typeof shippablePrs>;
  onShip: (prs: PrRef[]) => void;
}) {
  const key = (c: (typeof candidates)[number]) =>
    `${c.repoKey}#${c.status.pr_number}`;
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const toggle = (k: string) =>
    setChosen((prev) => {
      const next = new Set(prev);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      return next;
    });
  return (
    <>
      <div className="context-menu-label">Which PRs are shipping</div>
      {candidates.map((c) => (
        <label key={key(c)} className="ship-picker-option">
          <input
            type="checkbox"
            checked={chosen.has(key(c))}
            onChange={() => toggle(key(c))}
          />
          {key(c)}
        </label>
      ))}
      <div className="context-menu-sep" />
      <button
        type="button"
        role="menuitem"
        disabled={chosen.size === 0}
        onClick={() =>
          onShip(
            candidates
              .filter((c) => chosen.has(key(c)))
              .map((c) => ({ repo_key: c.repoKey, number: c.status.pr_number })),
          )
        }
      >
        Ship
      </button>
    </>
  );
}

const TILE_TONE: Record<ShippingSignal, "green" | "yellow" | "red" | "gray"> = {
  no_reviewer: "red",
  ready: "green",
  merge_queue: "green",
  review: "yellow",
  ci_failing: "yellow",
  conflict: "yellow",
  ci_pending: "gray",
  unknown: "gray",
};

function SignalIcon({ signal }: { signal: ShippingSignal }) {
  switch (signal) {
    case "ready":
      return <CheckIcon />;
    case "merge_queue":
      return <MergeQueueIcon />;
    case "review":
      return <ReviewIcon />;
    case "no_reviewer":
      return <NoReviewerIcon />;
    case "ci_failing":
    case "ci_pending":
      return <CiIcon />;
    case "conflict":
      return <ConflictIcon />;
    case "unknown":
      return <span className="shipping-tile-glyph">?</span>;
  }
}

function NoReviewerIcon() {
  return (
    <svg className="gh-sq-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M1.5 8 C 3.5 4.5, 5.5 3, 8 3 C 10.5 3, 12.5 4.5, 14.5 8 C 12.5 11.5, 10.5 13, 8 13 C 5.5 13, 3.5 11.5, 1.5 8 Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="8" r="2.2" fill="currentColor" />
      <path
        d="M2.5 2.5 L13.5 13.5"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg className="gh-sq-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M3.5 8.5 L6.6 11.5 L12.5 4.8"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ConflictIcon() {
  return (
    <svg className="gh-sq-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M8 2.2 L14.2 13.2 H1.8 Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
      <path
        d="M8 6.4 V9.4"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <circle cx="8" cy="11.3" r="0.9" fill="currentColor" />
    </svg>
  );
}

function ShippingTile({
  workspace,
  selected,
  working,
  mini = false,
  onSelect,
  onContextMenu,
}: {
  workspace: Workspace;
  selected: boolean;
  working: boolean;
  /** Drawn inside a parent's row, where a click mustn't reach the row. */
  mini?: boolean;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
}) {
  let tone: string;
  let title: string;
  let icon: React.ReactNode;
  if (workspace.shipment?.kind === "shipping") {
    const prs = shippingPrs(workspace);
    const signal = shippingSignal(prs);
    tone = TILE_TONE[signal];
    title = shippingSummary(workspace, prs);
    icon = <SignalIcon signal={signal} />;
  } else {
    tone = "shipped";
    title = `${workspace.branch}\nshipped, ready to delete`;
    icon = <MergedIcon />;
  }
  return (
    <button
      type="button"
      className={[
        "shipping-tile",
        `tone-${tone}`,
        mini ? "mini" : "",
        selected ? "selected" : "",
        working ? "working" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      title={title}
      onPointerDown={mini ? (e) => e.stopPropagation() : undefined}
      onClick={(e) => {
        e.stopPropagation();
        onSelect();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        onContextMenu(e.clientX, e.clientY);
      }}
    >
      {icon}
    </button>
  );
}

const SHIPPING_EXPANDED_KEY = "tethys.shippingExpanded";

function ShippingArea({
  shipping,
  selectedId,
  workspaceWorking,
  onSelect,
  onContextMenu,
}: {
  shipping: Workspace[];
  selectedId: WorkspaceId | null;
  workspaceWorking: (ws: Workspace) => boolean;
  onSelect: (id: WorkspaceId) => void;
  onContextMenu: (ws: Workspace, x: number, y: number) => void;
}) {
  // Expanded names each workspace and its PRs, for finding a particular one.
  const [expanded, setExpanded] = useState(
    () => localStorage.getItem(SHIPPING_EXPANDED_KEY) === "1",
  );
  const toggle = () =>
    setExpanded((prev) => {
      localStorage.setItem(SHIPPING_EXPANDED_KEY, prev ? "0" : "1");
      return !prev;
    });

  if (shipping.length === 0) return null;

  const tile = (w: Workspace) => (
    <ShippingTile
      workspace={w}
      selected={w.id === selectedId}
      working={w.shipment?.kind === "shipping" && workspaceWorking(w)}
      onSelect={() => onSelect(w.id)}
      onContextMenu={(x, y) => onContextMenu(w, x, y)}
    />
  );

  return (
    <div className={`shipping-area${expanded ? " expanded" : ""}`}>
      <div className="shipping-header">
        <span className="shipping-label">Shipping</span>
        <button
          type="button"
          className="shipping-toggle"
          title={expanded ? "Minimize" : "Maximize"}
          aria-label={expanded ? "Minimize" : "Maximize"}
          onClick={toggle}
        >
          {expanded ? "−" : "+"}
        </button>
      </div>
      {expanded ? (
        <ul className="shipping-rows">
          {shipping.map((w) => (
            <li
              key={w.id}
              className={w.id === selectedId ? "selected" : undefined}
              onClick={() => onSelect(w.id)}
              onContextMenu={(e) => {
                e.preventDefault();
                onContextMenu(w, e.clientX, e.clientY);
              }}
            >
              {tile(w)}
              <span className="shipping-row-text">
                <span className="shipping-row-branch" title={w.branch}>
                  {w.branch}
                </span>
                <span className="shipping-row-prs">{prNumbers(w)}</span>
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <div className="shipping-grid">
          {shipping.map((w) => (
            <Fragment key={w.id}>{tile(w)}</Fragment>
          ))}
        </div>
      )}
    </div>
  );
}

/** A Shipped workspace no longer names its Shipping PRs; its tracked ones stand in. */
function prNumbers(w: Workspace): string {
  const numbers =
    w.shipment?.kind === "shipping"
      ? w.shipment.prs.map((p) => p.number)
      : w.repo_links.flatMap((l) => l.prs.map((p) => p.number));
  return numbers.map((n) => `#${n}`).join(" ");
}

function Progress({ done, total }: { done: number; total: number }) {
  return (
    <span className="workspace-progress" title={`${done} of ${total} done`}>
      <span className="workspace-progress-bar">
        <span style={{ width: `${(100 * done) / total}%` }} />
      </span>
      {done}/{total}
    </span>
  );
}

function Spinner() {
  return <span className="spinner" aria-hidden="true" />;
}
