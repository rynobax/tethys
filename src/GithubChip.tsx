import { openUrl } from "@tauri-apps/plugin-opener";
import type { ChecksRollup, GithubPrStatus, MergeQueueState } from "./types";
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

function DraftIcon() {
  return (
    <svg className="gh-sq-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M3 13 L3.7 10.5 L10.5 3.7 L12.3 5.5 L5.5 12.3 Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M9.1 5.1 L10.9 6.9"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
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

/** A train of entries advancing out of a queue. Deliberately unlike the merge
    glyph next to it: being queued is the state right before merging, so the two
    are worth telling apart at a glance. */
function MergeQueueIcon() {
  return (
    <svg className="gh-state-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M4 3.2 V13"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        fill="none"
      />
      <circle cx="4" cy="3.8" r="1.7" fill="currentColor" />
      <circle cx="4" cy="8.2" r="1.7" fill="currentColor" />
      <circle cx="4" cy="12.6" r="1.7" fill="currentColor" />
      <path
        d="M6.6 3.8 H11.6 M10 2.2 L11.6 3.8 L10 5.4"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
        fill="none"
      />
    </svg>
  );
}

/** Octicon git-merge: the chip's whole story once a PR lands. */
function MergedIcon() {
  return (
    <svg className="gh-state-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        fill="currentColor"
        d="M5.45 5.154A4.25 4.25 0 0 0 9.25 7.5h1.378a2.251 2.251 0 1 1 0 1.5H9.25A5.734 5.734 0 0 1 5 7.123v3.505a2.25 2.25 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.95-.218ZM4.25 13.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5Zm8.5-4.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5ZM5 3.25a.75.75 0 1 0 0 .005V3.25Z"
      />
    </svg>
  );
}

/** Octicon git-pull-request-closed. */
function ClosedIcon() {
  return (
    <svg className="gh-state-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        fill="currentColor"
        d="M3.25 1A2.25 2.25 0 0 1 4 5.372v5.256a2.251 2.251 0 1 1-1.5 0V5.372A2.251 2.251 0 0 1 3.25 1Zm9.5 5.5a.75.75 0 0 1 .75.75v3.378a2.251 2.251 0 1 1-1.5 0V7.25a.75.75 0 0 1 .75-.75Zm-2.03-5.273a.75.75 0 0 1 1.06 0l.97.97.97-.97a.748.748 0 0 1 1.265.332.75.75 0 0 1-.205.729l-.97.97.97.968a.75.75 0 1 1-1.06 1.06l-.97-.968-.97.969a.75.75 0 0 1-1.06-1.06l.97-.97-.97-.97a.75.75 0 0 1 0-1.06ZM2.5 13.25a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0ZM3.25 2.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm9.5 10.75a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Z"
      />
    </svg>
  );
}

function Square({
  kind,
  tone,
  title,
}: {
  kind: "ci" | "review" | "draft" | "bugbot";
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
      {kind === "draft" && <DraftIcon />}
      {kind === "bugbot" && <BugbotIcon />}
    </span>
  );
}

/** Red once the queue is about to give up on the PR — those two need acting
    on, where the rest of the queue is just waiting. */
function mergeQueueTone(state: MergeQueueState): "pending" | "bad" {
  return state === "unmergeable" || state === "locked" ? "bad" : "pending";
}

function mergeQueueTitle(state: MergeQueueState): string {
  switch (state) {
    case "queued":
      return "In the merge queue, waiting its turn";
    case "awaiting_checks":
      return "In the merge queue: checks running on the merge branch";
    case "mergeable":
      return "In the merge queue: checks passed, merging shortly";
    case "unmergeable":
      return "In the merge queue, about to be ejected: the merge branch failed";
    case "locked":
      return "In the merge queue, held by a lock on the base branch";
  }
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
  /** When set, renders a detach button. The sidebar leaves it off — detaching
   *  is a workspace-header action — but any tracked PR can be detached. */
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
    status.merge_queue ? mergeQueueTitle(status.merge_queue) : null,
    status.last_error ? `error: ${status.last_error}` : null,
    stale ? `stale since ${new Date(status.fetched_at).toLocaleTimeString()}` : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <span className={classes} title={baseTitle} onClick={onClick}>
      <span className="gh-pr">{status.pr_number}</span>
      {isOpen && (
        <span className="gh-squares">
          <Square
            kind="ci"
            tone={ciTone(status.checks, status.has_merge_conflicts)}
            title={ciTitle(status.checks, status.has_merge_conflicts)}
          />
          {status.is_draft ? (
            <Square kind="draft" tone="gray" title="Draft: not ready for review" />
          ) : (
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
      {status.merge_queue && (
        <span
          className={`gh-state-badge gh-queue-${mergeQueueTone(status.merge_queue)}`}
          title={mergeQueueTitle(status.merge_queue)}
          aria-label={mergeQueueTitle(status.merge_queue)}
        >
          <MergeQueueIcon />
        </span>
      )}
      {status.state === "merged" && (
        <span className="gh-state-badge" title="Merged" aria-label="Merged">
          <MergedIcon />
        </span>
      )}
      {status.state === "closed" && (
        <span
          className="gh-state-badge gh-closed-badge"
          title="Closed without merging"
          aria-label="Closed without merging"
        >
          <ClosedIcon />
        </span>
      )}
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
