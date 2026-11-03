//! A repo's `[repo.env]` table, rendered for one worktree. The same pairs go
//! into the worktree's `.env`, where tools like Docker Compose read them, and
//! into the environment of its setup and teardown scripts.
//!
//! Rendering depends only on the worktree path, so purge computes exactly what
//! provisioning wrote — even for a worktree made before the table existed,
//! whose `.env` never got the lines.

use std::path::Path;

use crate::error::AppResult;
use crate::job::JobTx;
use crate::registry::Repo;

pub type EnvVars = Vec<(String, String)>;

pub fn render(repo: &Repo, worktree_path: &Path) -> EnvVars {
    let workspace = worktree_path
        .parent()
        .and_then(Path::file_name)
        .map(|n| slug(&n.to_string_lossy()))
        .unwrap_or_default();
    repo.env
        .iter()
        .map(|(key, value)| {
            let value = value
                .replace("{workspace}", &workspace)
                .replace("{repo}", &repo.key);
            (key.clone(), value)
        })
        .collect()
}

/// Compose project names are the strictest consumer: lowercase letters,
/// digits, `-` and `_`.
fn slug(name: &str) -> String {
    name.chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '_' | '-') => c,
            _ => '-',
        })
        .collect()
}

/// Leaves a worktree without an `.env` alone when there is nothing to write.
pub async fn write_dotenv(
    worktree_path: &Path,
    vars: &EnvVars,
    tx: &JobTx,
    repo_key: &str,
) -> AppResult<()> {
    if vars.is_empty() {
        return Ok(());
    }
    let path = worktree_path.join(".env");
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    tokio::fs::write(&path, upsert(&existing, vars)).await?;
    let keys: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
    tx.status(format!("wrote {} to .env", keys.join(", ")), Some(repo_key));
    Ok(())
}

/// Every line already setting one of `vars` is replaced in place; the rest are
/// appended. Other lines, comments included, are kept as they were.
fn upsert(existing: &str, vars: &EnvVars) -> String {
    let mut written = vec![false; vars.len()];
    let mut out = String::with_capacity(existing.len());
    for line in existing.lines() {
        match vars.iter().position(|(k, _)| line_key(line) == Some(k.as_str())) {
            Some(i) => {
                out.push_str(&assignment(&vars[i].0, &vars[i].1));
                written[i] = true;
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    for ((key, value), done) in vars.iter().zip(written) {
        if !done {
            out.push_str(&assignment(key, value));
            out.push('\n');
        }
    }
    out
}

fn line_key(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let line = line.strip_prefix("export ").unwrap_or(line);
    line.split_once('=').map(|(key, _)| key.trim())
}

/// Single quotes make a dotenv value literal: no `#` comment, no `$`
/// interpolation. Registry validation keeps `'` itself out of values.
fn assignment(key: &str, value: &str) -> String {
    let plain = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | ','));
    if plain {
        format!("{key}={value}")
    } else {
        format!("{key}='{value}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> EnvVars {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn renders_the_workspace_as_a_compose_safe_name() {
        let repo: Repo = toml::from_str(
            r#"
            key = "nl-backend"
            remote_url = "x"
            env = { COMPOSE_PROJECT_NAME = "{repo}-{workspace}" }
            "#,
        )
        .unwrap();
        let rendered = render(&repo, Path::new("/wt/Fix.Thing-nov-3/nl-backend"));
        assert_eq!(
            rendered,
            vars(&[("COMPOSE_PROJECT_NAME", "nl-backend-fix-thing-nov-3")])
        );
    }

    #[test]
    fn replaces_existing_lines_and_appends_the_rest() {
        let existing = "# local\nA=1\nexport COMPOSE_PROJECT_NAME=nl-backend\nB=2";
        let out = upsert(
            existing,
            &vars(&[("COMPOSE_PROJECT_NAME", "nl-backend-ws"), ("NEW", "x")]),
        );
        assert_eq!(
            out,
            "# local\nA=1\nCOMPOSE_PROJECT_NAME=nl-backend-ws\nB=2\nNEW=x\n"
        );
    }

    #[test]
    fn a_prefix_of_another_key_is_not_a_match() {
        let out = upsert("COMPOSE_PROJECT_NAME_X=1\n", &vars(&[("COMPOSE_PROJECT_NAME", "a")]));
        assert_eq!(out, "COMPOSE_PROJECT_NAME_X=1\nCOMPOSE_PROJECT_NAME=a\n");
    }

    #[test]
    fn quotes_values_dotenv_would_otherwise_interpret() {
        assert_eq!(assignment("K", "a b #c $D"), "K='a b #c $D'");
        assert_eq!(assignment("K", "nl-backend-ws"), "K=nl-backend-ws");
    }
}
