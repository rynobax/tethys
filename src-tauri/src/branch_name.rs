//! Picking a free branch name for a new workspace.
//!
//! What has to be unique is the workspace *directory*, not the git branch: two
//! workspaces sharing one would mean deleting either clobbers the other. A
//! branch that already exists locally is deliberately checked out as-is (that's
//! how you pick up a PR branch), so it isn't a conflict here.
//!
//! A taken name gets today's date first (`my-foo-sep-28`), because a clash is
//! usually an old workspace for the same idea and the date is what tells the
//! two apart in the sidebar. Only if that's taken too does a counter go on the
//! end (`my-foo-sep-28-2`).

use std::path::Path;

use chrono::NaiveDate;

use crate::error::{AppError, AppResult};
use crate::inprogress::InProgressWorkspaces;
use crate::registry;

/// How many `-2`, `-3`… suffixes to try on the dated name before giving up.
const MAX_COUNTER: u32 = 50;

/// A branch name and the workspace directory it maps to.
pub struct Reserved {
    pub branch: String,
    pub workspace_dir: String,
}

/// Find a branch name whose workspace directory is free under `worktree_root`.
///
/// A directory being provisioned right now — including one parked in the
/// setup queue — doesn't exist on disk yet, so the in-progress set is checked
/// too; otherwise two creates landing at once would both pick the same name.
/// That still leaves the instant between picking a name and provisioning
/// registering it, which is why provisioning's own on-disk check stays.
pub fn reserve(
    worktree_root: &Path,
    in_progress: &InProgressWorkspaces,
    requested: &str,
) -> AppResult<Reserved> {
    let provisioning = in_progress.snapshot();
    let today = chrono::Local::now().date_naive();
    pick(requested, today, |dir| {
        provisioning.contains(dir) || worktree_root.join(dir).exists()
    })
}

fn pick(requested: &str, today: NaiveDate, taken: impl Fn(&str) -> bool) -> AppResult<Reserved> {
    candidates(requested, today)
        .map(|branch| Reserved {
            workspace_dir: registry::sanitize_branch_for_dir(&branch),
            branch,
        })
        .find(|r| !taken(&r.workspace_dir))
        .ok_or_else(|| {
            AppError::Other(format!(
                "no free workspace directory for `{requested}` after {MAX_COUNTER} attempts"
            ))
        })
}

/// `my-foo`, `my-foo-sep-28`, `my-foo-sep-28-2`, `my-foo-sep-28-3`, …
///
/// A name that already ends in today's date skips straight to the counter, so
/// retrying `my-foo-sep-28` doesn't propose `my-foo-sep-28-sep-28`.
fn candidates(requested: &str, today: NaiveDate) -> impl Iterator<Item = String> + '_ {
    let date = today.format("%b-%-d").to_string().to_lowercase();
    let dated = if requested.ends_with(&format!("-{date}")) {
        None
    } else {
        Some(format!("{requested}-{date}"))
    };
    let base = dated.clone().unwrap_or_else(|| requested.to_string());
    std::iter::once(requested.to_string())
        .chain(dated)
        .chain((2..=MAX_COUNTER).map(move |n| format!("{base}-{n}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sep_28() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()
    }

    fn picked(requested: &str, taken: &[&str]) -> String {
        pick(requested, sep_28(), |dir| taken.contains(&dir))
            .unwrap()
            .branch
    }

    #[test]
    fn free_name_is_used_as_is() {
        assert_eq!(picked("my-foo", &[]), "my-foo");
    }

    #[test]
    fn taken_name_gets_the_date() {
        assert_eq!(picked("my-foo", &["my-foo"]), "my-foo-sep-28");
    }

    #[test]
    fn taken_dated_name_gets_a_counter() {
        assert_eq!(
            picked("my-foo", &["my-foo", "my-foo-sep-28"]),
            "my-foo-sep-28-2"
        );
        assert_eq!(
            picked("my-foo", &["my-foo", "my-foo-sep-28", "my-foo-sep-28-2"]),
            "my-foo-sep-28-3"
        );
    }

    #[test]
    fn day_is_not_zero_padded() {
        let oct_3 = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let r = pick("x", oct_3, |dir| dir == "x").unwrap();
        assert_eq!(r.branch, "x-oct-3");
    }

    #[test]
    fn already_dated_name_skips_to_the_counter() {
        assert_eq!(picked("my-foo-sep-28", &["my-foo-sep-28"]), "my-foo-sep-28-2");
    }

    #[test]
    fn collision_is_judged_on_the_directory_not_the_branch() {
        // `feat/x` and `feat-x` share a directory.
        let r = pick("feat/x", sep_28(), |dir| dir == "feat-x").unwrap();
        assert_eq!(r.branch, "feat/x-sep-28");
        assert_eq!(r.workspace_dir, "feat-x-sep-28");
    }

    #[test]
    fn gives_up_eventually() {
        assert!(pick("x", sep_28(), |_| true).is_err());
    }
}
