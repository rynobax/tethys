import { useEffect, useState } from "react";
import * as api from "./ipc/commands";
import { useAppEvent } from "./ipc/events";
import type { GithubAuthSnapshot } from "./types";

export function GithubAuthFooter() {
  const [snap, setSnap] = useState<GithubAuthSnapshot | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.githubAuthStatus()
      .then(setSnap)
      .catch(() => setSnap(null));
  }, []);

  useAppEvent("github:auth_changed", setSnap);

  if (!snap) return null;

  const reprobe = async () => {
    setBusy(true);
    try {
      const next = await api.githubReprobeAuth();
      setSnap(next);
    } finally {
      setBusy(false);
    }
  };

  const dotClass =
    snap.state === "authenticated"
      ? "gh-auth-dot-ok"
      : snap.state === "disabled"
        ? "gh-auth-dot-off"
        : "gh-auth-dot-warn";

  const label =
    snap.state === "authenticated"
      ? `@${snap.login ?? "?"}`
      : snap.state === "not_authenticated"
        ? "sign in"
        : snap.state === "disabled"
          ? "gh missing"
          : "checking…";

  const title =
    snap.state === "authenticated"
      ? `GitHub: authenticated as @${snap.login}`
      : snap.state === "not_authenticated"
        ? "GitHub: not authenticated. Run `gh auth login` in your terminal, then click to re-check."
        : snap.state === "disabled"
          ? "GitHub CLI (`gh`) is not installed. PR status is disabled."
          : "Checking GitHub auth…";

  return (
    <button
      type="button"
      className="gh-auth-footer"
      onClick={reprobe}
      disabled={busy || snap.state === "disabled"}
      title={title}
    >
      <span className={`gh-auth-dot ${dotClass}`} />
      <span className="gh-auth-label">{label}</span>
    </button>
  );
}
