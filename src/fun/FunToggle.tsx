import { useCallback, useState } from "react";

const STORAGE_KEY = "tethys.funMode";

export function useFunMode(): [boolean, () => void] {
  const [on, setOn] = useState(() => localStorage.getItem(STORAGE_KEY) === "1");
  const toggle = useCallback(() => {
    setOn((prev) => {
      localStorage.setItem(STORAGE_KEY, prev ? "0" : "1");
      return !prev;
    });
  }, []);
  return [on, toggle];
}

/** Bottom-right corner; in fun mode it also stands in for "New workspace". */
export function FunControls({
  on,
  onToggle,
  onNewWorkspace,
  canCreate,
}: {
  on: boolean;
  onToggle: () => void;
  onNewWorkspace: () => void;
  canCreate: boolean;
}) {
  return (
    <div className={on ? "fun-controls on" : "fun-controls"}>
      {on && (
        <button
          type="button"
          className="fun-control"
          onClick={onNewWorkspace}
          disabled={!canCreate}
          title="New workspace"
        >
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            <path
              d="M8 3v10M3 8h10"
              stroke="currentColor"
              strokeWidth="1.8"
              strokeLinecap="round"
            />
          </svg>
        </button>
      )}
      <button
        type="button"
        className="fun-control"
        onClick={onToggle}
        title={on ? "Back to the list" : "Fun mode"}
        aria-pressed={on}
      >
        {on ? (
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            <path
              d="M3 4.5h10M3 8h10M3 11.5h10"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
            />
          </svg>
        ) : (
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            <path
              d="M8 14V8M8 8c0-2.5-1.8-4.2-4.5-4.2C3.5 6.3 5.3 8 8 8Zm0 0c0-2.5 1.8-4.2 4.5-4.2C12.5 6.3 10.7 8 8 8Z"
              fill="currentColor"
              stroke="currentColor"
              strokeWidth="1.2"
              strokeLinejoin="round"
            />
          </svg>
        )}
      </button>
    </div>
  );
}
