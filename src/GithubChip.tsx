import { openUrl } from "@tauri-apps/plugin-opener";
import type { ChecksRollup, GithubPrStatus } from "./types";
import { isStale } from "./workspaceDerived";

type SquareTone = "green" | "yellow" | "red" | "gray";

function CiIcon() {
  return (
    <svg className="gh-sq-icon" viewBox="0 0 16 16" aria-hidden="true">
      <circle
        cx="8"
        cy="8"
        r="5.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
      />
      <path
        d="M8 4.5 L8 8 L10.4 9.6"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ReviewIcon() {
  return (
    <svg className="gh-sq-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M1.5 8 C 3.5 4.5, 5.5 3, 8 3 C 10.5 3, 12.5 4.5, 14.5 8 C 12.5 11.5, 10.5 13, 8 13 C 5.5 13, 3.5 11.5, 1.5 8 Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="8" r="2.2" fill="currentColor" />
    </svg>
  );
}

function BugbotIcon() {
  return (
    <svg className="gh-sq-icon" viewBox="0 0 16 16" aria-hidden="true">
      <ellipse cx="8" cy="9" rx="3.4" ry="4" fill="currentColor" />
      <path
        d="M8 5 L8 3.2 M5.2 5.5 L3.8 4.2 M10.8 5.5 L12.2 4.2"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        fill="none"
      />
      <path
        d="M4.2 8 L2.6 7.4 M11.8 8 L13.4 7.4 M4.2 10.5 L2.6 10.8 M11.8 10.5 L13.4 10.8"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        fill="none"
      />
    </svg>
  );
}

function Square({
  kind,
  tone,
  title,
}: {
  kind: "ci" | "review" | "bugbot";
  tone: SquareTone;
  title: string;
}) {
  return (
    <span
      className={`gh-sq gh-sq-${kind} gh-sq-tone-${tone}`}
      title={title}
      aria-label={title}
    >
      {kind === "ci" && <CiIcon />}
      {kind === "review" && <ReviewIcon />}
      {kind === "bugbot" && <BugbotIcon />}
    </span>
  );
}

function ciTone(checks: ChecksRollup, hasMergeConflicts: boolean): SquareTone {
  if (hasMergeConflicts) return "red";
  switch (checks) {
    case "success":
    case "neutral":
      return "green";
    case "failure":
      return "red";
    case "pending":
      return "yellow";
    case "none":
      return "gray";
  }
}

function reviewTone(
  decision: GithubPrStatus["review_decision"],
  unresolved: number,
): SquareTone {
  switch (decision) {
    case "approved":
      // Approved with unresolved threads is still "feedback outstanding".
      return unresolved > 0 ? "yellow" : "green";
    case "changes_requested":
      return "red";
    case "review_required":
      return "gray";
    case "none":
      return unresolved > 0 ? "yellow" : "gray";
  }
}

function bugbotTone(bugbot: ChecksRollup): SquareTone {
  switch (bugbot) {
    case "success":
    case "neutral":
      return "green";
    case "failure":
      return "red";
    case "pending":
      return "yellow";
    case "none":
      return "gray";
  }
}

function ciTitle(checks: ChecksRollup, hasMergeConflicts: boolean): string {
  if (hasMergeConflicts) return "Merge conflict with base branch";
  switch (checks) {
    case "success":
      return "CI: passing";
    case "failure":
      return "CI: failing";
    case "pending":
      return "CI: running";
    case "neutral":
      return "CI: neutral";
    case "none":
      return "CI: no checks";
  }
}

function reviewTitle(
  decision: GithubPrStatus["review_decision"],
  unresolved: number,
): string {
  const base = (() => {
    switch (decision) {
      case "approved":
        return "Review: approved";
      case "changes_requested":
        return "Review: changes requested";
      case "review_required":
        return "Review: waiting on review";
      case "none":
        return "Review: no reviewers";
    }
  })();
  return unresolved > 0 ? `${base} · ${unresolved} unresolved` : base;
}

function bugbotTitle(bugbot: ChecksRollup): string {
  switch (bugbot) {
    case "success":
      return "Bugbot: clean";
    case "failure":
      return "Bugbot: issues found";
    case "pending":
      return "Bugbot: running";
    case "neutral":
      return "Bugbot: neutral";
    case "none":
      return "Bugbot: not run";
  }
}

export function GithubChip({
  status,
  linkable = true,
  context,
  onDetach,
}: {
  status: GithubPrStatus;
  /** When false, the chip is informational only — no click-to-open, no hover. */
  linkable?: boolean;
  /**
   * Extra hover context, prepended to the tooltip. The sidebar passes the repo
   * key, since it drops the repo label from the row and the chip is then the
   * only place that attribution can live.
   */
  context?: string;
  /** When set, renders a detach button. Only for manually-attached PRs. */
  onDetach?: () => void;
}) {
  const stale = isStale(status.fetched_at);
  const isOpen = status.state === "open";

  const onClick = linkable
    ? (e: React.MouseEvent) => {
        e.stopPropagation();
        openUrl(status.url).catch(() => {
          /* non-fatal */
        });
      }
    : undefined;

  const classes = [
    "gh-chip",
    `gh-state-${status.state}`,
    stale ? "gh-stale" : "",
    status.is_draft ? "gh-draft" : "",
    linkable ? "" : "gh-chip-static",
  ]
    .filter(Boolean)
    .join(" ");

  const baseTitle = [
    context,
    `PR #${status.pr_number}`,
    status.head_branch,
    `state: ${status.state}${status.is_draft ? " (draft)" : ""}`,
    status.last_error ? `error: ${status.last_error}` : null,
    stale ? `stale since ${new Date(status.fetched_at).toLocaleTimeString()}` : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <span className={classes} title={baseTitle} onClick={onClick}>
      <span className="gh-pr">#{status.pr_number}</span>
      {isOpen && (
        <span className="gh-squares">
          <Square
            kind="ci"
            tone={ciTone(status.checks, status.has_merge_conflicts)}
            title={ciTitle(status.checks, status.has_merge_conflicts)}
          />
          {!status.is_draft && (
            <Square
              kind="review"
              tone={reviewTone(status.review_decision, status.unresolved_threads)}
              title={reviewTitle(status.review_decision, status.unresolved_threads)}
            />
          )}
          <Square
            kind="bugbot"
            tone={bugbotTone(status.bugbot)}
            title={bugbotTitle(status.bugbot)}
          />
        </span>
      )}
      {status.state === "merged" && (
        <span className="gh-merged-badge">merged</span>
      )}
      {status.state === "closed" && (
        <span className="gh-merged-badge gh-closed-badge">closed</span>
      )}
      {status.is_draft && <span className="gh-draft-badge">draft</span>}
      {onDetach && (
        <PrDetachButton prNumber={status.pr_number} onDetach={onDetach} />
      )}
    </span>
  );
}

/** Removes a manually-attached PR from the workspace. Nothing on GitHub changes. */
export function PrDetachButton({
  prNumber,
  onDetach,
}: {
  prNumber: number;
  onDetach: () => void;
}) {
  const label = `Stop tracking PR #${prNumber}`;
  return (
    <button
      type="button"
      className="gh-detach"
      title={label}
      aria-label={label}
      onClick={(e) => {
        e.stopPropagation();
        onDetach();
      }}
    >
      ×
    </button>
  );
}
