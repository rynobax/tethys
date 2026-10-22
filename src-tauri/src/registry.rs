use std::path::{Path, PathBuf};

use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult};
use crate::github::{parse_github_remote, GithubSlug};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RepoRegistry {
    /// Absolute directory where Tethys creates per-workspace worktrees. Prefer
    /// a path without spaces; macOS "Application Support" paths in particular
    /// tend to break naive setup scripts.
    pub worktree_root: PathBuf,

    /// One entry per repo you want Tethys-managed workspaces to span.
    #[serde(default, rename(serialize = "repos", deserialize = "repo"))]
    #[schemars(rename = "repo")]
    pub repos: Vec<Repo>,

    /// Overrides for the `CLAUDE.md` generated at each workspace root.
    #[serde(default)]
    pub workspace_doc: Option<WorkspaceDocConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WorkspaceDocConfig {
    /// Markdown body of the generated workspace `CLAUDE.md`. Tethys still
    /// prepends its marker line and appends a "Repo notes" section from each
    /// repo's `claude_notes`.
    ///
    /// Placeholders: `{branch}`, `{repo_list}`, `{available_repos}` (registry
    /// repos not in this workspace), `{workspace_root}`, `{clone_dir}`. Any
    /// other `{token}` is left alone, so braces need no escaping.
    pub body: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Repo {
    /// Unique, stable id used in `state.json`, workspace paths, and as the UI
    /// label. Lowercase + dashes recommended.
    pub key: String,

    /// Git remote URL. Tethys keeps its own clone under
    /// `<data_dir>/repos/<key>/`, never touching your own checkouts.
    pub remote_url: String,

    /// Branch new worktrees are cut from. Defaults to `origin/HEAD`.
    #[serde(default)]
    pub default_branch: Option<String>,

    /// Shell command run in each new worktree (e.g. `pnpm install`).
    #[serde(default)]
    pub default_setup_script: Option<String>,

    /// Setup script timeout in seconds. Defaults to 600.
    #[serde(default)]
    pub setup_timeout_secs: Option<u64>,

    /// Files copied from the base clone into each new worktree, relative to
    /// the clone root (e.g. `.env`). Missing sources are skipped and existing
    /// destinations left alone; absolute paths and `..` are rejected.
    #[serde(default)]
    pub copy_files: Vec<String>,

    /// Markdown rendered verbatim into the generated `CLAUDE.md` of every
    /// workspace with this repo: local-setup context you'd otherwise re-explain
    /// each session. Guidance about the code belongs in the repo's own
    /// `CLAUDE.md`.
    #[serde(default)]
    pub claude_notes: Option<String>,

    /// `None` for a remote that isn't on github.com.
    #[serde(skip, default)]
    #[schemars(skip)]
    pub github_slug: Option<GithubSlug>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistryLoad {
    Ok {
        path: PathBuf,
        registry: RepoRegistry,
    },
    Missing {
        path: PathBuf,
    },
    Invalid {
        path: PathBuf,
        error: String,
    },
}

impl RegistryLoad {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => match toml::from_str::<RepoRegistry>(&s) {
                Ok(mut registry) => {
                    if let Err(e) = validate_worktree_root(&registry.worktree_root) {
                        warn!(error = %e, "repos.toml loaded but worktree_root is unusable");
                        return RegistryLoad::Invalid {
                            path: path.to_path_buf(),
                            error: format!("worktree_root: {e}"),
                        };
                    }
                    populate_github_slugs(&mut registry);
                    info!(
                        repos = registry.repos.len(),
                        worktree_root = %registry.worktree_root.display(),
                        "repos.toml loaded",
                    );
                    RegistryLoad::Ok {
                        path: path.to_path_buf(),
                        registry,
                    }
                }
                Err(e) => RegistryLoad::Invalid {
                    path: path.to_path_buf(),
                    error: e.to_string(),
                },
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegistryLoad::Missing {
                path: path.to_path_buf(),
            },
            Err(e) => RegistryLoad::Invalid {
                path: path.to_path_buf(),
                error: e.to_string(),
            },
        }
    }

    pub fn require(&self) -> AppResult<&RepoRegistry> {
        match self {
            RegistryLoad::Ok { registry, .. } => Ok(registry),
            RegistryLoad::Missing { path } => Err(AppError::Other(format!(
                "repos.toml is missing at {}",
                path.display()
            ))),
            RegistryLoad::Invalid { path, error } => Err(AppError::Other(format!(
                "repos.toml at {} is invalid: {error}",
                path.display()
            ))),
        }
    }
}

impl RepoRegistry {
    pub fn find_repo(&self, key: &str) -> Option<&Repo> {
        self.repos.iter().find(|r| r.key == key)
    }

    pub fn workspace_doc_body(&self) -> &str {
        self.workspace_doc
            .as_ref()
            .map(|c| c.body.trim())
            .filter(|b| !b.is_empty())
            .unwrap_or(crate::workspace_doc::DEFAULT_BODY)
    }

    pub fn plan_worktree_path(&self, workspace_dir: &str, repo_key: &str) -> PathBuf {
        self.worktree_root.join(workspace_dir).join(repo_key)
    }
}

/// Git refs already forbid every other path-unsafe character.
pub fn sanitize_branch_for_dir(branch: &str) -> String {
    branch.replace('/', "-")
}

fn populate_github_slugs(registry: &mut RepoRegistry) {
    for repo in &mut registry.repos {
        repo.github_slug = parse_github_remote(&repo.remote_url);
        if repo.github_slug.is_none() {
            info!(
                key = %repo.key,
                remote = %repo.remote_url,
                "repo has non-GitHub remote; skipping GitHub sync for this repo",
            );
        }
    }
}

fn validate_worktree_root(root: &Path) -> AppResult<()> {
    std::fs::create_dir_all(root).map_err(|e| {
        AppError::Other(format!(
            "could not create {}: {e}",
            root.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(AppError::Other(format!(
            "{} exists but is not a directory",
            root.display()
        )));
    }
    let probe = root.join(".tethys-write-probe");
    std::fs::write(&probe, b"ok").map_err(|e| {
        AppError::Other(format!("{} is not writable: {e}", root.display()))
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

pub fn write_schema(path: &Path) -> AppResult<()> {
    let schema = schema_for!(RepoRegistry);
    let json = serde_json::to_string_pretty(&schema)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)?;
    debug!(?path, "wrote repos.schema.json");
    Ok(())
}

pub fn starter_template() -> &'static str {
    r##"#:schema ./repos.schema.json
# Tethys repo registry. Add one [[repo]] block per repo you want to work on.
#
# worktree_root: where Tethys creates per-workspace worktrees. Prefer a path
# with no spaces (e.g. ~/code/tethys-worktrees) to avoid quoting hazards in
# setup scripts.
#
# Each repo is cloned on first use into <data_dir>/repos/<key>, then every
# worktree is `git worktree add`ed from that clone — Tethys never touches
# any checkout you maintain separately.

worktree_root = "CHANGE_ME_TO_AN_ABSOLUTE_PATH"

# [[repo]]
# key = "frontend"
# remote_url = "git@github.com:me/frontend.git"
# default_branch = "main"          # defaults to the remote's default branch
# default_setup_script = "pnpm install"
# setup_timeout_secs = 600
# copy_files = [".env", "apps/web/.env.local"]
#
# # Standing context for Claude, rendered into each workspace's generated
# # CLAUDE.md. Good for local-setup facts you'd otherwise repeat every session.
# claude_notes = """
# A global django container is usually already running — don't start another.
# """

# Tethys generates a CLAUDE.md at each workspace root explaining the worktree
# layout. Uncomment to replace its wording with your own. Placeholders:
# {branch} {repo_list} {available_repos} {workspace_root} {clone_dir}.
#
# [workspace_doc]
# body = """
# # Workspace `{branch}`
#
# Repos checked out here:
#
# {repo_list}
#
# Everything you may touch is under `{workspace_root}`. Need a repo that isn't
# here? Ask me to add it — don't go read another checkout.
# """
"##
}
