import { useCallback, useRef } from "react";

/**
 * Makes a single-row strip (tabs, chips) scroll sideways under an ordinary
 * mouse wheel. A trackpad already swipes horizontally, but a wheel only
 * produces `deltaY`, which a horizontally overflowing element ignores — so
 * the strip looked stuck to anyone not on a trackpad.
 *
 * Returns a callback ref for the scrolling element, so it keeps working when
 * that element is conditionally rendered (a collapsed panel, a header that
 * appears once the workspace loads). Only intercepts when the element
 * actually overflows and the gesture is mostly vertical, so a trackpad's own
 * horizontal swipe is left alone.
 */
export function useHorizontalScroll<T extends HTMLElement>() {
  const cleanup = useRef<(() => void) | null>(null);
  return useCallback((el: T | null) => {
    cleanup.current?.();
    cleanup.current = null;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (el.scrollWidth <= el.clientWidth) return;
      if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
      el.scrollLeft += e.deltaY;
      e.preventDefault();
    };
    // Non-passive so `preventDefault` stops the page under it from scrolling.
    el.addEventListener("wheel", onWheel, { passive: false });
    cleanup.current = () => el.removeEventListener("wheel", onWheel);
  }, []);
}
