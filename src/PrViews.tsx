import {
  createContext,
  useContext,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";

/** Across all workspaces. Each is a renderer process of tens of megabytes. */
const MAX_LIVE = 6;

interface Slot {
  url: string;
  host: HTMLElement;
}

const SlotContext = createContext<{
  slot: Slot | null;
  setSlot: (slot: Slot | null) => void;
}>({ slot: null, setSlot: () => {} });

export function PrViewsProvider({ children }: { children: ReactNode }) {
  const [slot, setSlot] = useState<Slot | null>(null);
  return (
    <SlotContext.Provider value={{ slot, setSlot }}>
      {children}
    </SlotContext.Provider>
  );
}

/** Lays the PR's page over `hostRef` while the caller is mounted. */
export function usePrViewSlot(
  url: string,
  hostRef: RefObject<HTMLElement | null>,
) {
  const { setSlot } = useContext(SlotContext);
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    setSlot({ url, host });
    return () => setSlot(null);
  }, [url, hostRef, setSlot]);
}

/**
 * Every live PR page, one per URL, kept loaded while hidden so page and scroll
 * survive switching tabs and workspaces. Mounted once, above anything that
 * comes and goes, because a `<webview>` reloads whenever its element is
 * removed or moved: so the list is never reordered, only appended to and
 * evicted from. Each is fixed over its tab's host rather than rendered in it,
 * and hidden with `visibility`, since a `<webview>` under `display: none`
 * loses its size.
 */
export function PrViews() {
  const { slot } = useContext(SlotContext);
  const [live, setLive] = useState<{ url: string; shownAt: number }[]>([]);
  const clock = useRef(0);
  const elements = useRef(new Map<string, HTMLElement>());

  const url = slot?.url;
  useLayoutEffect(() => {
    if (!url) return;
    const shownAt = ++clock.current;
    setLive((prev) => {
      let next = prev.some((p) => p.url === url)
        ? prev.map((p) => (p.url === url ? { url, shownAt } : p))
        : [...prev, { url, shownAt }];
      while (next.length > MAX_LIVE) {
        const oldest = next.reduce((a, b) => (a.shownAt < b.shownAt ? a : b));
        next = next.filter((p) => p !== oldest);
      }
      return next;
    });
  }, [url]);

  useLayoutEffect(() => {
    for (const [u, el] of elements.current) {
      if (u !== slot?.url) el.style.visibility = "hidden";
    }
    const el = slot && elements.current.get(slot.url);
    if (!slot || !el) return;
    const place = () => {
      const r = slot.host.getBoundingClientRect();
      Object.assign(el.style, {
        left: `${r.left}px`,
        top: `${r.top}px`,
        width: `${r.width}px`,
        height: `${r.height}px`,
        visibility: r.width < 2 || r.height < 2 ? "hidden" : "visible",
      });
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(slot.host);
    window.addEventListener("resize", place);
    // A scrolled ancestor moves the host without resizing it.
    document.addEventListener("scroll", place, true);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
      document.removeEventListener("scroll", place, true);
    };
  }, [slot, live]);

  return (
    <div className="pr-views">
      {live.map(({ url }) => (
        <webview
          key={url}
          ref={(el: HTMLElement | null) => {
            if (el) elements.current.set(url, el);
            else elements.current.delete(url);
          }}
          className="pr-view"
          src={url}
          partition="persist:github"
          // React drops a `true` it doesn't know to be boolean. Popups are
          // let through only so main's window-open handler sees them.
          {...({ allowpopups: "" } as object)}
        />
      ))}
    </div>
  );
}
