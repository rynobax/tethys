import { useEffect, useState } from "react";
import * as api from "./ipc/commands";
import { useAppEvent } from "./ipc/events";
import type { GithubAuthSnapshot } from "./types";

/** Only there when `gh` is signed out, since PR status silently stops then. */
export function GithubAuthFooter() {
  const [snap, setSnap] = useState<GithubAuthSnapshot | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.githubAuthStatus()
      .then(setSnap)
      .catch(() => setSnap(null));
  }, []);

  useAppEvent("github:auth_changed", setSnap);

  if (snap?.state !== "not_authenticated") return null;

  const reprobe = async () => {
    setBusy(true);
    try {
      setSnap(await api.githubReprobeAuth());
    } finally {
      setBusy(false);
    }
  };

  return (
    <button
      type="button"
      className="gh-auth-footer"
      onClick={reprobe}
      disabled={busy}
      title="Run `gh auth login` in your terminal, then click to re-check."
    >
      <span className="gh-auth-dot" />
      GitHub signed out
    </button>
  );
}
