//! Marking a workspace's directory trusted in `~/.codex/config.toml`.
//!
//! The one thing about a codex session Tethys cannot express on the command
//! line. Everything else — hooks, MCP, sandbox, the doc filename — rides on
//! `-c` overrides, which is the arrangement that keeps the user's codex config
//! theirs. Trust is the exception, and not for want of trying: `-c
//! projects."<dir>".trust_level="trusted"` parses and is accepted, and the
//! prompt still appears, because the check reads persisted config rather than
//! the session layer. There is no flag for it either.
//!
//! Left alone, codex asks "Do you trust the contents of this directory?" on
//! the first run in any new directory — and every workspace is a new
//! directory, so it would be every workspace, every time, on a screen nobody
//! is watching when the session was started by a handoff.
//!
//! What trust buys codex is the right to load project-local `.codex/config.toml`,
//! `.codex/hooks.json` and `.rules` out of the checkout. That is a real
//! widening: a repo could ship those. It is the same trust Tethys already
//! extends by cloning the repo and pointing an agent at it, and it is scoped
//! to worktrees Tethys made.
//!
//! Written with `toml_edit` so the user's comments, ordering and formatting
//! survive, under the same advisory `flock` discipline as the Claude hook
//! install, and removed again at purge so the file doesn't accumulate a stanza
//! per workspace forever.

use std::fs::OpenOptions;
use std::path::Path;

use fs2::FileExt;
use toml_edit::{DocumentMut, Item, Table, value};
use tracing::{debug, warn};

use crate::error::{AppError, AppResult};

/// Mark `dir` trusted. Idempotent, and a no-op if it already is.
pub fn trust(config_path: &Path, lock_path: &Path, dir: &Path) -> AppResult<()> {
    edit(config_path, lock_path, dir, true)
}

/// Drop `dir`'s entry. Called at purge: the directory is gone, so the stanza
/// is only ever going to be noise in someone's config file.
pub fn untrust(config_path: &Path, lock_path: &Path, dir: &Path) -> AppResult<()> {
    edit(config_path, lock_path, dir, false)
}

fn edit(config_path: &Path, lock_path: &Path, dir: &Path, trusted: bool) -> AppResult<()> {
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    lock_file.lock_exclusive()?;

    let result = edit_inner(config_path, dir, trusted);

    FileExt::unlock(&lock_file).ok();
    drop(lock_file);
    result
}

fn edit_inner(config_path: &Path, dir: &Path, trusted: bool) -> AppResult<()> {
    let key = project_key(dir);

    let existing = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc: DocumentMut = existing.parse().map_err(|e| {
        AppError::Other(format!(
            "{} is not valid TOML: {e}",
            config_path.display()
        ))
    })?;

    let projects = doc
        .entry("projects")
        .or_insert_with(|| Item::Table(implicit_table()))
        .as_table_mut()
        .ok_or_else(|| AppError::Other("codex config `projects` is not a table".into()))?;
    projects.set_implicit(true);

    let already = projects
        .get(&key)
        .and_then(Item::as_table_like)
        .and_then(|t| t.get("trust_level"))
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == "trusted");

    if trusted {
        if already {
            return Ok(());
        }
        projects
            .entry(&key)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_like_mut()
            .ok_or_else(|| {
                AppError::Other(format!("codex config `projects.{key}` is not a table"))
            })?
            .insert("trust_level", value("trusted"));
    } else {
        if projects.remove(&key).is_none() {
            return Ok(());
        }
        // Don't leave an empty `[projects]` behind in a file that never had one.
        if projects.is_empty() {
            doc.remove("projects");
        }
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(config_path, doc.to_string().as_bytes())?;
    debug!(path = %config_path.display(), dir = %key, trusted, "codex project trust updated");
    Ok(())
}

/// The key codex files a directory under: its resolved path.
///
/// Resolution matters on macOS, where `/tmp` and `/private/tmp` are different
/// projects and only one is the one codex will look up. It has to work on a
/// path that no longer exists, though — purge removes the worktree before it
/// cleans the config — so this resolves the deepest ancestor that *does*
/// exist and re-joins the rest. The parent worktrees directory outlives any
/// one workspace, which is what makes the answer the same before and after.
fn project_key(dir: &Path) -> String {
    let mut suffix = Vec::new();
    let mut cursor = dir;
    loop {
        if let Ok(resolved) = std::fs::canonicalize(cursor) {
            let mut out = resolved;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out.to_string_lossy().into_owned();
        }
        match (cursor.file_name(), cursor.parent()) {
            (Some(name), Some(parent)) => {
                suffix.push(name.to_os_string());
                cursor = parent;
            }
            // Nothing along the path resolves; the literal path is the best
            // key available, and for a removal it's better than giving up.
            _ => return dir.to_string_lossy().into_owned(),
        }
    }
}

/// A `[projects]` table that only prints if it has children, so a config that
/// never needed one doesn't grow an empty header.
fn implicit_table() -> Table {
    let mut t = Table::new();
    t.set_implicit(true);
    t
}

/// Write via a sibling temp file and rename, so a crash mid-write can't leave
/// the user with a truncated codex config.
fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let tmp = path.with_extension("toml.tethys-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Trust `dir`, logging rather than failing.
///
/// A session that can't be pre-trusted still runs — it just asks — so this is
/// never a reason to refuse to start one.
pub fn trust_or_warn(paths: &crate::paths::Paths, dir: &Path) {
    let Some(config) = crate::paths::codex_config_path() else {
        warn!("no HOME — cannot mark codex project trusted");
        return;
    };
    if let Err(e) = trust(&config, &paths.codex_config_lock(), dir) {
        warn!(error = %e, dir = %dir.display(), "could not mark codex project trusted");
    }
}

/// Drop `dir`'s trust entry, logging rather than failing.
pub fn untrust_or_warn(paths: &crate::paths::Paths, dir: &Path) {
    let Some(config) = crate::paths::codex_config_path() else { return };
    if let Err(e) = untrust(&config, &paths.codex_config_lock(), dir) {
        warn!(error = %e, dir = %dir.display(), "could not drop codex project trust");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(initial: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        let lock = tmp.path().join("lock");
        let project = tmp.path().join("ws");
        std::fs::create_dir_all(&project).unwrap();
        if !initial.is_empty() {
            std::fs::write(&config, initial).unwrap();
        }
        (tmp, config, lock, project)
    }

    fn key(project: &Path) -> String {
        super::project_key(project)
    }

    #[test]
    fn trusting_a_directory_writes_the_stanza() {
        let (_t, config, lock, project) = setup("");
        trust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains(&format!("[projects.\"{}\"]", key(&project))));
        assert!(written.contains(r#"trust_level = "trusted""#));
    }

    /// The user's config is theirs: comments and unrelated keys have to come
    /// back out the other side untouched, which is why this uses `toml_edit`
    /// and not a serde round-trip.
    #[test]
    fn everything_else_in_the_file_survives() {
        let initial = "# my notes\nmodel = \"gpt-5.6-terra\"\n\n[tui]\nalternate_screen = \"never\"\n";
        let (_t, config, lock, project) = setup(initial);
        trust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains("# my notes"));
        assert!(written.contains("model = \"gpt-5.6-terra\""));
        assert!(written.contains("[tui]"));
        assert!(written.contains("alternate_screen = \"never\""));
    }

    /// Every boot re-runs this for every codex workspace, so it has to be a
    /// no-op the second time rather than a growing pile of stanzas.
    #[test]
    fn trusting_twice_changes_nothing() {
        let (_t, config, lock, project) = setup("");
        trust(&config, &lock, &project).unwrap();
        let once = std::fs::read_to_string(&config).unwrap();
        trust(&config, &lock, &project).unwrap();
        assert_eq!(once, std::fs::read_to_string(&config).unwrap());
    }

    /// Purge removes the directory; the stanza has to go with it or the file
    /// accumulates one per workspace forever.
    #[test]
    fn untrusting_removes_the_stanza_and_its_empty_table() {
        let (_t, config, lock, project) = setup("model = \"gpt-5.6-terra\"\n");
        trust(&config, &lock, &project).unwrap();
        untrust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(!written.contains("trust_level"));
        assert!(!written.contains("[projects]"));
        assert!(written.contains("model = \"gpt-5.6-terra\""));
    }

    /// A project the user trusted by hand, or another workspace's, must not be
    /// removed along with ours.
    #[test]
    fn untrusting_leaves_other_projects_alone() {
        let initial = "[projects.\"/Users/ryan/code/elsewhere\"]\ntrust_level = \"trusted\"\n";
        let (_t, config, lock, project) = setup(initial);
        trust(&config, &lock, &project).unwrap();
        untrust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains("/Users/ryan/code/elsewhere"));
        assert!(!written.contains(&key(&project)));
    }

    /// Purge runs after the worktree is gone, so the path can't be resolved
    /// directly — the key has to come out the same anyway, or the entry is
    /// orphaned in the user's config forever.
    #[test]
    fn untrusting_works_after_the_directory_is_deleted() {
        let (_t, config, lock, project) = setup("");
        trust(&config, &lock, &project).unwrap();
        let expected = key(&project);
        std::fs::remove_dir_all(&project).unwrap();
        untrust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(!written.contains(&expected), "{written}");
    }

    /// The same directory has to produce the same key whether or not it still
    /// exists, since it's written before the worktree is removed and deleted
    /// after.
    #[test]
    fn the_key_survives_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("ws");
        std::fs::create_dir_all(&project).unwrap();
        let while_present = super::project_key(&project);
        std::fs::remove_dir_all(&project).unwrap();
        assert_eq!(while_present, super::project_key(&project));
        // And it really was resolved, not just echoed back.
        assert_ne!(while_present, project.to_string_lossy());
    }

    /// A config Tethys can't parse is the user's to fix. Better to report it
    /// and leave the file alone than to overwrite what we couldn't read.
    #[test]
    fn a_broken_config_is_reported_not_overwritten() {
        let (_t, config, lock, project) = setup("this is [not valid toml\n");
        assert!(trust(&config, &lock, &project).is_err());
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "this is [not valid toml\n"
        );
    }
}
