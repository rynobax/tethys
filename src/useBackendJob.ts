import { useEffect, useRef, useState } from "react";
import { Channel } from "./ipc/commands";

import type { JobEvent } from "./types";

export type JobState = "running" | "success" | "failed";

/** Runs a backend job exactly once per `key`. */
export function useBackendJob(
  key: string,
  start: (onEvent: Channel<JobEvent>) => Promise<unknown>,
  onSuccess: () => void,
): { events: JobEvent[]; state: JobState } {
  const [events, setEvents] = useState<JobEvent[]>([]);
  const [state, setState] = useState<JobState>("running");
  const startedKeyRef = useRef<string | null>(null);
  const startRef = useRef(start);
  startRef.current = start;
  const onSuccessRef = useRef(onSuccess);
  onSuccessRef.current = onSuccess;

  useEffect(() => {
    if (startedKeyRef.current === key) return;
    startedKeyRef.current = key;
    setEvents([]);
    setState("running");

    const channel = new Channel<JobEvent>();
    channel.onmessage = (event) => {
      if (startedKeyRef.current !== key) return;
      setEvents((prev) => [...prev, event]);
      if (event.kind === "success") setState("success");
      else if (event.kind === "failed") setState("failed");
    };

    startRef
      .current(channel)
      .then(() => {
        if (startedKeyRef.current !== key) return;
        setState((s) => (s === "running" ? "success" : s));
        onSuccessRef.current();
      })
      .catch((e) => {
        if (startedKeyRef.current !== key) return;
        setEvents((prev) => {
          const last = prev[prev.length - 1];
          if (last?.kind === "failed") return prev;
          return [...prev, { kind: "failed", error: String(e) }];
        });
        setState((s) => (s === "running" ? "failed" : s));
      });
  }, [key]);

  return { events, state };
}
