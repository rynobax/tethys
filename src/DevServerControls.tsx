import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

import type {
  BeMode,
  DevServersMeta,
  DevStateSnapshot,
  MemoryPressure,
  MemorySnapshot,
  Workspace,
} from "./types";
import { useTauriEvent } from "./useTauriEvent";

/**
 * Per-workspace dev-server controls.
 *
 *   <DevServerActions />        → header `.actions` div. Build local /
 *                                 Stop / Open FE / Open BE / Open admin.
 *                                 Self-subscribes to pressure so the
 *                                 "Build local" click can gate on it.
 *
 *   <SidebarMemoryPressure />   → bottom of the sidebar footer. System
 *                                 pressure level + free %. Caller passes
 *                                 the latest snapshot down.
 *
 *   <SidebarWorkspaceMemoryChip /> → per-row inline. Renders the FE +
 *                                 BE RAM totals for a workspace when
 *                                 its dev servers are running.
 */

/** Memory-pressure-gated start: refuses on Critical, runs on Normal/Warning. */
function useStartGate(workspaceId: string, onChanged: () => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingCritical, setPendingCritical] = useState(false);
  // Held outside React state so click handlers see the latest pressure
  // without re-binding every poll tick.
  const pressureRef = useRef<MemoryPressure>("normal");

  useTauriEvent<MemorySnapshot>("devserver:memory_updated", (e) => {
    pressureRef.current = e.payload.system.level;
  });
  useEffect(() => {
    invoke<MemorySnapshot>("get_memory_snapshot")
      .then((s) => {
        pressureRef.current = s.system.level;
      })
      .catch(() => {});
  }, []);

  const start = useCallback(
    async (mode: BeMode, bypassGate = false) => {
      if (!bypassGate && pressureRef.current === "critical") {
        setPendingCritical(true);
        return;
      }
      setError(null);
      setBusy(true);
      try {
        await invoke<DevServersMeta>("start_dev_servers", {
          args: { workspace_id: workspaceId, mode },
        });
        onChanged();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [workspaceId, onChanged],
  );

  return {
    busy,
    error,
    setError,
    start,
    pendingCritical,
    dismissCritical: () => setPendingCritical(false),
    pressure: pressureRef,
  };
}

/** Live dev-server snapshot for one workspace; refreshed when meta changes. */
function useDevState(workspace: Workspace): DevStateSnapshot | null {
  const dev = workspace.dev_servers;
  const [state, setState] = useState<DevStateSnapshot | null>(null);
  useEffect(() => {
    if (!dev) {
      setState(null);
      return;
    }
    let cancelled = false;
    invoke<DevStateSnapshot>("get_dev_state", {
      args: { workspace_id: workspace.id },
    })
      .then((s) => {
        if (!cancelled) setState(s);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [workspace.id, dev]);
  return state;
}

// ── Header action buttons ────────────────────────────────────────────

export function DevServerActions({
  workspace,
  onChanged,
}: {
  workspace: Workspace;
  /** Refresh the workspace list. */
  onChanged: () => void;
}) {
  const dev = workspace.dev_servers;
  const live = useDevState(workspace);
  const gate = useStartGate(workspace.id, onChanged);
  const feLive = live?.fe?.running ?? false;
  const beLive = live?.be?.running ?? false;

  const stop = useCallback(async () => {
    gate.setError(null);
    try {
      await invoke("stop_dev_servers", {
        args: { workspace_id: workspace.id },
      });
      onChanged();
    } catch (e) {
      gate.setError(String(e));
    }
  }, [workspace.id, onChanged, gate]);

  const openFe = useCallback(() => {
    if (!dev) return;
    openUrl(`http://localhost:${dev.fe_port}/`).catch((e) =>
      console.error("openUrl FE:", e),
    );
  }, [dev]);
  const openBe = useCallback(
    (admin: boolean) => {
      if (!dev?.be_port) return;
      const path = admin ? "/admin/" : "/";
      openUrl(`http://localhost:${dev.be_port}${path}`).catch((e) =>
        console.error("openUrl BE:", e),
      );
    },
    [dev],
  );

  const proxyLabel = (() => {
    const t = live?.fe_proxy_target ?? dev?.fe_proxy_target ?? null;
    if (!t) return null;
    return t.includes(":8000") ? "master" : "this worktree";
  })();

  return (
    <>
      {!feLive && (
        <button
          type="button"
          onClick={() => gate.start("auto")}
          disabled={gate.busy}
          title={
            gate.pressure.current === "warning"
              ? "Memory pressure elevated — Build local will proceed"
              : "Build FE + auto-decide BE"
          }
        >
          {gate.pressure.current === "warning" && "⚠ "}Build local
        </button>
      )}
      {feLive && dev && (
        <>
          <button
            type="button"
            onClick={openFe}
            title={
              proxyLabel
                ? `Open http://localhost:${dev.fe_port}/  (API → ${proxyLabel})`
                : `Open http://localhost:${dev.fe_port}/`
            }
          >
            ↗ FE :{dev.fe_port}
          </button>
          {beLive && dev.be_port !== null ? (
            <button
              type="button"
              onClick={() => openBe(true)}
              title={`Open http://localhost:${dev.be_port}/admin/`}
            >
              ↗ BE :{dev.be_port}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => gate.start("force_include")}
              disabled={gate.busy}
              title="Spin up a worktree django alongside the FE"
            >
              Start BE
            </button>
          )}
          <button
            type="button"
            className="danger"
            onClick={stop}
            disabled={gate.busy}
            title="Stop dev servers"
          >
            ■ Stop
          </button>
        </>
      )}
      {gate.error && <span className="dev-error-inline">{gate.error}</span>}
      {gate.pendingCritical && (
        <CriticalPressureModal
          onCancel={gate.dismissCritical}
          onConfirm={() => {
            gate.dismissCritical();
            gate.start("auto", true);
          }}
        />
      )}
    </>
  );
}

// ── Sidebar footer: system memory pressure ───────────────────────────

/** Mirror the `SystemStatus` button shape: stoplight dot + label, same
 *  font + sizing. Non-interactive (no modal yet) — a plain div, not a
 *  button, but the visual treatment matches so it sits cleanly next
 *  to "Status" / "Auth" rows. */
export function SidebarMemoryPressure({
  memory,
}: {
  memory: MemorySnapshot | null;
}) {
  if (!memory) return null;
  const { system } = memory;
  const stoplight =
    system.level === "critical"
      ? "red"
      : system.level === "warning"
        ? "yellow"
        : "green";
  const used_mib = Math.max(0, system.total_mib - system.free_mib);
  const used_pct = 100 - system.free_pct;
  return (
    <div
      className="system-status-button memory-row"
      title={`${pressureWord(system.level)} RAM pressure · ${formatGiB(used_mib)} used of ${formatGiB(system.total_mib)} · ${formatGiB(system.free_mib)} free`}
    >
      <span className={`status-dot ${stoplight}`} />
      <span className="memory-row-text">
        RAM · {formatGiBValue(used_mib)} / {formatGiB(system.total_mib)} · {used_pct}%
      </span>
    </div>
  );
}

/** MiB -> "5.2 GiB" / "16 GiB" — drops the decimal when it's whole. */
function formatGiB(mib: number): string {
  return `${formatGiBValue(mib)} GiB`;
}

/** Same number formatting as `formatGiB` but without the unit — used
 *  in the fraction's numerator where the denominator carries the unit.
 *  Drops the decimal only when the value is effectively a whole
 *  number; otherwise renders one decimal place. */
function formatGiBValue(mib: number): string {
  const gib = mib / 1024;
  if (Math.abs(gib - Math.round(gib)) < 0.05) {
    return `${Math.round(gib)}`;
  }
  return gib.toFixed(1);
}

// ── Sidebar row: per-workspace RAM chip ──────────────────────────────

/**
 * Sub-row for the sidebar's workspace entry — renders beneath the
 * repo list when this workspace's dev servers are running. Shows
 * summed FE + BE RAM in GiB and as % of system total. Caller already
 * holds the snapshot so we don't subscribe here.
 */
export function SidebarWorkspaceMemoryChip({
  workspace,
  memory,
}: {
  workspace: Workspace;
  memory: MemorySnapshot | null;
}) {
  if (!workspace.dev_servers || !memory) return null;
  const entry = memory.per_workspace.find(
    (w) => w.workspace_id === workspace.id,
  );
  if (!entry) return null;
  const total_mib = entry.fe_mib + entry.be_mib;
  if (total_mib === 0) return null;
  const pct_of_system =
    memory.system.total_mib > 0
      ? Math.round((total_mib * 100) / memory.system.total_mib)
      : 0;
  const parts: string[] = [];
  if (entry.fe_mib > 0) parts.push(`FE ${formatGiB(entry.fe_mib)}`);
  if (entry.be_mib > 0) parts.push(`BE ${formatGiB(entry.be_mib)}`);
  return (
    <span
      className="workspace-mem-chip"
      title={`Dev servers: ${parts.join(" + ")}  ·  ${formatGiB(total_mib)} of ${formatGiB(memory.system.total_mib)} total system RAM`}
    >
      RAM · {formatGiB(total_mib)} · {pct_of_system}%
    </span>
  );
}

// ── Critical-pressure confirm modal ──────────────────────────────────

function CriticalPressureModal({
  onCancel,
  onConfirm,
}: {
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="dev-modal-backdrop">
      <div className="dev-modal">
        <p>
          System memory is at <strong>Critical</strong> pressure. Starting
          another stack is likely to OOM.
        </p>
        <p>Stop a worktree first?</p>
        <div className="dev-modal-actions">
          <button onClick={onCancel}>I'll stop one manually</button>
          <button onClick={onConfirm}>Start anyway</button>
        </div>
      </div>
    </div>
  );
}

function pressureWord(p: MemoryPressure): string {
  switch (p) {
    case "normal":
      return "Normal";
    case "warning":
      return "⚠ Warning";
    case "critical":
      return "🛑 Critical";
    case "unknown":
      return "?";
  }
}
