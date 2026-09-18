import { useEffect, useRef } from "react";
import {
  listen,
  type EventCallback,
  type UnlistenFn,
} from "@tauri-apps/api/event";

/**
 * Subscribe to a Tauri event for the lifetime of the component.
 *
 * Handles two failure modes the naive `listen().then(un => un())` pattern
 * has in dev:
 *   - Cleanup fires before `listen()` resolves (StrictMode double-mount,
 *     fast re-renders): we defer the unlisten until the promise settles, and
 *     then by one more macrotask. `listen()` resolves with the id before the
 *     eval'd script that records the listener has run in the page, so an
 *     unlisten in that gap throws `listeners[eventId].handlerId` inside Tauri
 *     and never reaches the Rust side, leaking the listener.
 *   - `un()` rejects because Tauri's internal listener map was cleared out
 *     from under us (Vite HMR module re-evaluation): we swallow it — the
 *     listener is effectively gone either way. Tauri's unlisten is async, so
 *     a try/catch around the call would miss this; it has to be a `.catch`.
 *
 * The handler is captured in a ref so a new reference each render doesn't
 * re-subscribe.
 */
export function useTauriEvent<T>(
  event: string,
  handler: EventCallback<T>,
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    let disposed = false;
    let un: UnlistenFn | null = null;
    // Typed `() => void`, but it's an async function at runtime.
    const release = (fn: UnlistenFn) => {
      Promise.resolve(fn()).catch(() => {});
    };

    listen<T>(event, (e) => handlerRef.current(e))
      .then((fn) => {
        if (disposed) {
          setTimeout(() => release(fn), 0);
        } else {
          un = fn;
        }
      })
      .catch(() => {});

    return () => {
      disposed = true;
      if (un) release(un);
      un = null;
    };
  }, [event]);
}
