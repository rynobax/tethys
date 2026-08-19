import { useEffect, useRef, useState } from "react";
import { Channel, runJob } from "./ipc/commands";

import type { JobEvent } from "./types";

export type JobState = "running" | "success" | "failed";

export interface JobDescriptor {
  key: string;
  command: string;
  args: Record<string, unknown>;
}

interface UseBackendJobOptions {
  onSuccess?: (key: string, result: unknown) => void;
}

/**
 * Runs a backend job exactly once per `descriptor.key`. The invocation is
 * tied to the descriptor, not to any component's mount lifecycle, so callers
 * can freely mount and unmount the UI that displays the job without
 * re-firing it.
 */
export function useBackendJob(
  descriptor: JobDescriptor | null,
  options: UseBackendJobOptions = {},
): { events: JobEvent[]; state: JobState } {
  const [events, setEvents] = useState<JobEvent[]>([]);
  const [state, setState] = useState<JobState>("running");
  const startedKeyRef = useRef<string | null>(null);
  const onSuccessRef = useRef(options.onSuccess);
  onSuccessRef.current = options.onSuccess;

  const key = descriptor?.key ?? null;

  useEffect(() => {
    if (!descriptor) {
      startedKeyRef.current = null;
      setEvents([]);
      setState("running");
      return;
    }
    if (startedKeyRef.current === descriptor.key) return;
    startedKeyRef.current = descriptor.key;
    setEvents([]);
    setState("running");

    const capturedKey = descriptor.key;
    const channel = new Channel<JobEvent>();
    channel.onmessage = (event) => {
      if (startedKeyRef.current !== capturedKey) return;
      setEvents((prev) => [...prev, event]);
      if (event.kind === "success") setState("success");
      else if (event.kind === "failed") setState("failed");
    };

    runJob(descriptor.command, descriptor.args, channel)
      .then((res) => {
        if (startedKeyRef.current !== capturedKey) return;
        setState((s) => (s === "running" ? "success" : s));
        onSuccessRef.current?.(capturedKey, res);
      })
      .catch((e) => {
        if (startedKeyRef.current !== capturedKey) return;
        setEvents((prev) => {
          const last = prev[prev.length - 1];
          if (last?.kind === "failed") return prev;
          return [...prev, { kind: "failed", error: String(e) }];
        });
        setState((s) => (s === "running" ? "failed" : s));
      });
    // descriptor is keyed by `key`; command/args are captured on first run.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  return { events, state };
}
