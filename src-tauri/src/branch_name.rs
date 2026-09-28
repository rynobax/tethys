//! Uniqueness is judged on the workspace *directory*, not the git branch: an
//! existing local branch is checked out as-is, which is how a PR branch gets
//! picked up.
//!
//! The date goes on before a counter because a clash is usually an old
//! workspace for the same idea, and the date is what tells them apart.

use std::path::Path;

use chrono::NaiveDate;

use crate::error::{AppError, AppResult};
use crate::inprogress::InProgressWorkspaces;
use crate::registry;

const MAX_COUNTER: u32 = 50;

pub struct Reserved {
    pub branch: String,
    pub workspace_dir: String,
}

/// A queued or provisioning workspace has no directory yet, so the in-progress
/// set is checked too. Provisioning still re-checks disk to close the gap
/// before it registers.
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

/// `my-foo`, `my-foo-sep-28`, `my-foo-sep-28-2`, …; an already-dated name
/// skips straight to the counter.
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
