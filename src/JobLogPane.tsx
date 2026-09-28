import { useEffect, useRef } from "react";

import type { JobEvent } from "./types";
import type { JobState } from "./useBackendJob";

interface Props {
  title: string;
  events: JobEvent[];
  state: JobState;
  onDismiss: () => void;
}

export function JobLogPane({ title, events, state, onDismiss }: Props) {
  const logRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [events]);

  return (
    <div className="job-log-pane">
      <header className="job-log-header">
        <h3>{title}</h3>
        <span className={`job-state job-state-${state}`}>
          {state === "running" ? "running…" : state}
        </span>
      </header>
      <div className="job-log-lines" ref={logRef}>
        {events.length === 0 && (
          <div className="job-log-line status muted">Starting…</div>
        )}
        {events.map((e, i) => (
          <JobLogLine key={i} event={e} />
        ))}
      </div>
      <div className="job-log-actions">
        <button
          type="button"
          onClick={onDismiss}
          disabled={state === "running"}
          autoFocus={state !== "running"}
        >
          {state === "running"
            ? "Running…"
            : state === "success"
              ? "Done"
              : "Close"}
        </button>
      </div>
    </div>
  );
}

function JobLogLine({ event }: { event: JobEvent }) {
  switch (event.kind) {
    case "status":
      return (
        <div className="job-log-line status">
          {event.repo && <span className="repo-tag">{event.repo}</span>}
          <span className="status-text">{event.message}</span>
        </div>
      );
    case "log":
      return (
        <div className={`job-log-line log log-${event.stream}`}>
          {event.repo && <span className="repo-tag">{event.repo}</span>}
          <span className="log-text">{event.line}</span>
        </div>
      );
    case "success":
      return <div className="job-log-line status ok">✔ done</div>;
    case "failed":
      return <div className="job-log-line status err">✘ {event.error}</div>;
  }
}
