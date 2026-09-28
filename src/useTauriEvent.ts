import { useEffect, useRef } from "react";
import {
  listen,
  type EventCallback,
  type UnlistenFn,
} from "@tauri-apps/api/event";

/**
 * An unlisten issued right after `listen()` resolves runs before Tauri has
 * recorded the listener, throws, and leaks it — hence the extra macrotask.
 * Unlisten rejections are swallowed: after an HMR reload the listener is
 * already gone.
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
    // Typed `() => void`, but async at runtime.
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
