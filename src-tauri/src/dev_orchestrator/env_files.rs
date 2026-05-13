//! Propagate secret-holding files from the main stack into a worktree
//! BE dir. Symlinks (not copies) so rotations on the main `.env`
//! propagate to every worktree on next container restart. Also adds
//! each linked filename to the worktree's `.git/info/exclude` (local-
//! only gitignore) so it doesn't appear in `git status`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;

use tracing::warn;

use super::config::OrchestratorConfig;

pub fn link_into_worktree(cfg: &OrchestratorConfig, be_dir: &Path) -> std::io::Result<()> {
    for name in &cfg.env_symlinks {
        let src = cfg.main_stack_dir.join(name);
        let dst = be_dir.join(name);
        if src.exists() && !dst.exists() {
            symlink(&src, &dst)?;
        }
    }
    if let Err(e) = register_gitignore(be_dir, cfg) {
        // Non-fatal — gitignore is a hygiene-only concern; the dev
        // server still works without it.
        warn!(error = %e, "failed to register dev-files in .git/info/exclude");
    }
    Ok(())
}

/// Resolve the worktree's `.git/info/exclude` path. Git worktrees use
/// a `.git` *file* that points at `.git/worktrees/<name>` in the main
/// repo; non-worktree clones have `.git` as a directory. Handle both.
fn worktree_exclude_path(be_dir: &Path) -> Option<std::path::PathBuf> {
    let dot_git = be_dir.join(".git");
    if dot_git.is_file() {
        let contents = fs::read_to_string(&dot_git).ok()?;
        for line in contents.lines() {
            if let Some(rest) = line.strip_prefix("gitdir: ") {
                let gitdir = std::path::PathBuf::from(rest.trim());
                return Some(gitdir.join("info").join("exclude"));
            }
        }
        None
    } else if dot_git.is_dir() {
        Some(dot_git.join("info").join("exclude"))
    } else {
        None
    }
}

fn register_gitignore(be_dir: &Path, cfg: &OrchestratorConfig) -> std::io::Result<()> {
    let Some(exclude) = worktree_exclude_path(be_dir) else {
        return Ok(());
    };
    if let Some(parent) = exclude.parent() {
        fs::create_dir_all(parent)?;
    }
    // Read existing entries so we don't add duplicates.
    let existing = fs::read_to_string(&exclude).unwrap_or_default();
    let mut existing_lines: std::collections::HashSet<&str> = existing.lines().collect();
    let want = std::iter::once("docker-compose.override.yml")
        .chain(cfg.env_symlinks.iter().map(String::as_str))
        .collect::<Vec<_>>();
    let mut added: Vec<&str> = Vec::new();
    for w in &want {
        if !existing_lines.contains(w) {
            added.push(w);
            existing_lines.insert(w);
        }
    }
    if added.is_empty() {
        return Ok(());
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    for line in added {
        writeln!(f, "{line}")?;
    }
    Ok(())
}
