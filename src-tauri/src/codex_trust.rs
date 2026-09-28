//! Codex asks whether to trust every new directory, and every workspace is
//! one. `-c projects."<dir>".trust_level` is accepted but ignored, since the
//! check reads persisted config, so this is the one thing written into
//! `~/.codex/config.toml` (via `toml_edit`, so the user's formatting survives).

use std::fs::OpenOptions;
use std::path::Path;

use fs2::FileExt;
use toml_edit::{DocumentMut, Item, Table, value};
use tracing::{debug, warn};

use crate::error::{AppError, AppResult};

pub fn trust(config_path: &Path, lock_path: &Path, dir: &Path) -> AppResult<()> {
    edit(config_path, lock_path, dir, true)
}

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

/// Codex keys on the resolved path (`/tmp` and `/private/tmp` differ). Purge
/// runs after the worktree is gone, so this resolves the deepest existing
/// ancestor and re-joins the rest.
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
            _ => return dir.to_string_lossy().into_owned(),
        }
    }
}

fn implicit_table() -> Table {
    let mut t = Table::new();
    t.set_implicit(true);
    t
}

fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let tmp = path.with_extension("toml.tethys-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// An untrusted session still runs; it just asks.
pub fn trust_or_warn(paths: &crate::paths::Paths, dir: &Path) {
    let Some(config) = crate::paths::codex_config_path() else {
        warn!("no HOME — cannot mark codex project trusted");
        return;
    };
    if let Err(e) = trust(&config, &paths.codex_config_lock(), dir) {
        warn!(error = %e, dir = %dir.display(), "could not mark codex project trusted");
    }
}

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

    #[test]
    fn trusting_a_directory_writes_the_stanza() {
        let (_t, config, lock, project) = setup("");
        trust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains(&format!("[projects.\"{}\"]", project_key(&project))));
        assert!(written.contains(r#"trust_level = "trusted""#));
    }

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

    #[test]
    fn trusting_twice_changes_nothing() {
        let (_t, config, lock, project) = setup("");
        trust(&config, &lock, &project).unwrap();
        let once = std::fs::read_to_string(&config).unwrap();
        trust(&config, &lock, &project).unwrap();
        assert_eq!(once, std::fs::read_to_string(&config).unwrap());
    }

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

    #[test]
    fn untrusting_leaves_other_projects_alone() {
        let initial = "[projects.\"/Users/ryan/code/elsewhere\"]\ntrust_level = \"trusted\"\n";
        let (_t, config, lock, project) = setup(initial);
        trust(&config, &lock, &project).unwrap();
        untrust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains("/Users/ryan/code/elsewhere"));
        assert!(!written.contains(&project_key(&project)));
    }

    #[test]
    fn untrusting_works_after_the_directory_is_deleted() {
        let (_t, config, lock, project) = setup("");
        trust(&config, &lock, &project).unwrap();
        let expected = project_key(&project);
        std::fs::remove_dir_all(&project).unwrap();
        untrust(&config, &lock, &project).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(!written.contains(&expected), "{written}");
    }

    #[test]
    fn the_key_survives_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("ws");
        std::fs::create_dir_all(&project).unwrap();
        let while_present = project_key(&project);
        std::fs::remove_dir_all(&project).unwrap();
        assert_eq!(while_present, project_key(&project));
        assert_ne!(while_present, project.to_string_lossy());
    }

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
