import { Component, useEffect, useRef, useState, type ReactNode } from "react";

interface CapturedError {
  id: number;
  source: "render" | "event" | "promise";
  message: string;
  stack?: string;
}

let nextId = 0;
const subscribers = new Set<(err: CapturedError) => void>();

function report(err: CapturedError) {
  subscribers.forEach((sub) => sub(err));
}

function toMessage(value: unknown): { message: string; stack?: string } {
  if (value instanceof Error) {
    return { message: value.message, stack: value.stack };
  }
  if (typeof value === "string") return { message: value };
  try {
    return { message: JSON.stringify(value) };
  } catch {
    return { message: String(value) };
  }
}

export class ErrorBoundary extends Component<
  { children: ReactNode },
  { hasError: boolean }
> {
  state = { hasError: false };

  static getDerivedStateFromError() {
    return { hasError: true };
  }

  componentDidCatch(error: unknown) {
    const { message, stack } = toMessage(error);
    report({ id: ++nextId, source: "render", message, stack });
    // Keep rendering; the error shows as a card instead.
    this.setState({ hasError: false });
  }

  render() {
    return this.props.children;
  }
}

export function ErrorOverlay() {
  const [errors, setErrors] = useState<CapturedError[]>([]);
  const recentRef = useRef(new Map<string, number>());

  useEffect(() => {
    const sub = (err: CapturedError) => {
      const key = `${err.source}:${err.message}`;
      const now = Date.now();
      const last = recentRef.current.get(key) ?? 0;
      if (now - last < 500) return;
      recentRef.current.set(key, now);

      setErrors((prev) => [...prev, err]);
    };
    subscribers.add(sub);

    const onError = (ev: ErrorEvent) => {
      const { message, stack } = toMessage(ev.error ?? ev.message);
      report({ id: ++nextId, source: "event", message, stack });
    };
    const onRejection = (ev: PromiseRejectionEvent) => {
      const { message, stack } = toMessage(ev.reason);
      report({ id: ++nextId, source: "promise", message, stack });
    };

    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);

    return () => {
      subscribers.delete(sub);
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  }, []);

  const dismiss = (id: number) =>
    setErrors((prev) => prev.filter((e) => e.id !== id));
  const dismissAll = () => setErrors([]);

  if (errors.length === 0) return null;

  return (
    <div className="error-overlay">
      {errors.length > 1 && (
        <button
          className="error-overlay-dismiss-all"
          type="button"
          onClick={dismissAll}
        >
          Dismiss all ({errors.length})
        </button>
      )}
      {errors.map((e) => (
        <div key={e.id} className="error-overlay-card">
          <div className="error-overlay-header">
            <span className="error-overlay-source">{e.source}</span>
            <button
              type="button"
              className="error-overlay-close"
              onClick={() => dismiss(e.id)}
              aria-label="Dismiss"
            >
              ×
            </button>
          </div>
          <div className="error-overlay-message">{e.message}</div>
          {e.stack && (
            <pre className="error-overlay-stack">{e.stack}</pre>
          )}
        </div>
      ))}
    </div>
  );
}
