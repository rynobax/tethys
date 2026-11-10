//! The workspace root's `.claude/settings.local.json`, which Tethys touches
//! only to let a sandboxed session run git: a worktree's metadata lives in its
//! clone's `.git` under the data dir, outside anything the sandbox lets a
//! session write by default.
//!
//! Everything else in the file is the session's, so it's read, extended and
//! written back with every key Tethys doesn't own left as it was.

use std::path::Path;

use serde_json::{Map, Value};
use tokio::fs;
use tracing::warn;

use crate::error::{AppError, AppResult};
use crate::paths::Paths;

/// Grants write access to each repo's clone `.git` dir. Idempotent, so the
/// same call covers a new workspace and a repo added to an existing one.
pub async fn grant_git_dirs(
    workspace_root: &Path,
    repo_keys: &[String],
    paths: &Paths,
) -> AppResult<()> {
    if !fs::try_exists(workspace_root).await? {
        return Ok(());
    }
    let file_path = workspace_root.join(".claude").join("settings.local.json");
    let mut doc = read(&file_path).await?;
    // Claude Code deep-merges sandbox config across scopes, so contribute only
    // these entries.
    let allow_write = array_at(
        object_at(object_at(&mut doc, "sandbox"), "filesystem"),
        "allowWrite",
    );
    for repo_key in repo_keys {
        let text = paths.repo_git_dir(repo_key).to_string_lossy().into_owned();
        if !allow_write.iter().any(|v| v.as_str() == Some(&text)) {
            allow_write.push(Value::String(text));
        }
    }
    write_atomic(&doc, &file_path).await
}

/// Missing or malformed reads as empty.
async fn read(path: &Path) -> AppResult<Map<String, Value>> {
    let raw = match fs::read_to_string(path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(AppError::Io(e)),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(m)) => Ok(m),
        _ => {
            warn!(path = %path.display(), "settings.local.json is not a JSON object — treating as empty");
            Ok(Map::new())
        }
    }
}

async fn write_atomic(doc: &Map<String, Value>, path: &Path) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut content = serde_json::to_string_pretty(doc)
        .map_err(|e| AppError::Other(format!("serializing settings.local.json: {e}")))?;
    content.push('\n');

    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, path).await?;
    Ok(())
}

/// Replaces a non-object value.
fn object_at<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let entry = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("just made an object")
}

/// Replaces a non-array value.
fn array_at<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    let entry = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !entry.is_array() {
        *entry = Value::Array(Vec::new());
    }
    entry.as_array_mut().expect("just made an array")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(tmp: &tempfile::TempDir) -> Paths {
        Paths {
            data_dir: tmp.path().to_path_buf(),
        }
    }

    async fn allow_write(workspace_root: &Path) -> Vec<String> {
        let raw = fs::read_to_string(workspace_root.join(".claude/settings.local.json"))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        parsed["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn git_dir(paths: &Paths, key: &str) -> String {
        paths.repo_git_dir(key).to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn grants_each_repos_git_dir_once() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let root = tmp.path().join("ws");
        fs::create_dir_all(&root).await.unwrap();

        grant_git_dirs(&root, &["frontend".into()], &paths).await.unwrap();
        grant_git_dirs(&root, &["frontend".into(), "backend".into()], &paths)
            .await
            .unwrap();

        assert_eq!(
            allow_write(&root).await,
            vec![git_dir(&paths, "frontend"), git_dir(&paths, "backend")]
        );
    }

    #[tokio::test]
    async fn keys_tethys_does_not_own_survive() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let root = tmp.path().join("ws");
        let file = root.join(".claude/settings.local.json");
        fs::create_dir_all(file.parent().unwrap()).await.unwrap();
        fs::write(
            &file,
            r#"{"model":"opus","permissions":{"allow":["Read(./a)"]},"sandbox":{"network":{"allowedDomains":["x.com"]}}}"#,
        )
        .await
        .unwrap();

        grant_git_dirs(&root, &["api".into()], &paths).await.unwrap();

        let back: Value =
            serde_json::from_str(&fs::read_to_string(&file).await.unwrap()).unwrap();
        assert_eq!(back["model"], "opus");
        assert_eq!(back["permissions"]["allow"], serde_json::json!(["Read(./a)"]));
        assert_eq!(
            back["sandbox"]["network"]["allowedDomains"],
            serde_json::json!(["x.com"])
        );
        assert_eq!(allow_write(&root).await, vec![git_dir(&paths, "api")]);
        assert!(!file.with_extension("json.tmp").exists());
    }

    #[tokio::test]
    async fn a_malformed_file_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(&tmp);
        let root = tmp.path().join("ws");
        let file = root.join(".claude/settings.local.json");
        fs::create_dir_all(file.parent().unwrap()).await.unwrap();
        fs::write(&file, "{ not json").await.unwrap();

        grant_git_dirs(&root, &["api".into()], &paths).await.unwrap();

        assert_eq!(allow_write(&root).await, vec![git_dir(&paths, "api")]);
    }

    #[tokio::test]
    async fn skips_when_workspace_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        grant_git_dirs(&missing, &["any".into()], &paths(&tmp))
            .await
            .unwrap();
        assert!(!missing.exists());
    }
}
