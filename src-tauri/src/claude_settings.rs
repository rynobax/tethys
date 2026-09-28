//! Claude Code's `settings.local.json` document format.

use std::path::Path;

use serde_json::{Map, Value};
use tokio::fs;
use tracing::warn;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCategory {
    Allow,
    Deny,
    Ask,
}

impl PermissionCategory {
    pub fn as_field(&self) -> &'static str {
        match self {
            PermissionCategory::Allow => "allow",
            PermissionCategory::Deny => "deny",
            PermissionCategory::Ask => "ask",
        }
    }

    /// In the order the workspace-root merge emits them.
    pub const ALL: [PermissionCategory; 3] = [
        PermissionCategory::Allow,
        PermissionCategory::Deny,
        PermissionCategory::Ask,
    ];
}

/// e.g. `Read(./src/**)`, `Bash(rm:*)`, `mcp__server__tool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionEntry {
    tool: String,
    arg: Option<String>,
    suffix: String,
}

impl PermissionEntry {
    /// An unrecognised entry is one with no argument.
    pub fn parse(entry: &str) -> Self {
        let no_arg = || Self {
            tool: entry.to_string(),
            arg: None,
            suffix: String::new(),
        };
        let (Some(open), Some(close)) = (entry.find('('), entry.rfind(')')) else {
            return no_arg();
        };
        if close <= open + 1 {
            return no_arg();
        }
        Self {
            tool: entry[..open].to_string(),
            arg: Some(entry[open + 1..close].to_string()),
            suffix: entry[close + 1..].to_string(),
        }
    }

    /// `Read(./src/**)` → `Read(./api/src/**)`. Only `./` arguments move.
    pub fn scoped_to_repo(&self, repo_key: &str) -> Self {
        let Some(rest) = self.arg.as_deref().and_then(|a| a.strip_prefix("./")) else {
            return self.clone();
        };
        Self {
            arg: Some(format!("./{repo_key}/{rest}")),
            ..self.clone()
        }
    }

    /// Inverse of [`PermissionEntry::scoped_to_repo`].
    pub fn unscope(&self, repo_keys: &[String]) -> Option<(String, Self)> {
        let rest = self.arg.as_deref()?.strip_prefix("./")?;
        for key in repo_keys {
            if let Some(remainder) = rest.strip_prefix(&format!("{key}/")) {
                return Some((
                    key.clone(),
                    Self {
                        arg: Some(format!("./{remainder}")),
                        ..self.clone()
                    },
                ));
            }
        }
        None
    }
}

impl std::fmt::Display for PermissionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.arg {
            Some(arg) => write!(f, "{}({}){}", self.tool, arg, self.suffix),
            None => write!(f, "{}", self.tool),
        }
    }
}

/// Keeps the raw object so keys Tethys doesn't own survive every edit.
#[derive(Debug, Default, Clone)]
pub struct SettingsDoc(Map<String, Value>);

impl SettingsDoc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Missing or malformed reads as empty. Malformed warns: a silently empty
    /// baseline turns every combined entry into a bogus Pending Permission.
    pub async fn read(path: &Path) -> AppResult<Self> {
        let raw = match fs::read_to_string(path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => {
                warn!(error = %e, path = %path.display(), "failed to read settings.local.json");
                return Err(AppError::Io(e));
            }
        };
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(m)) => Ok(Self(m)),
            Ok(_) => {
                warn!(path = %path.display(), "settings.local.json root is not an object — treating as empty");
                Ok(Self::new())
            }
            Err(e) => {
                warn!(error = %e, path = %path.display(), "settings.local.json is not valid JSON — treating as empty");
                Ok(Self::new())
            }
        }
    }

    pub async fn read_lossy(path: &Path) -> Self {
        Self::read(path).await.unwrap_or_default()
    }

    pub fn permissions(&self, category: PermissionCategory) -> Vec<PermissionEntry> {
        self.0
            .get("permissions")
            .and_then(Value::as_object)
            .and_then(|p| p.get(category.as_field()))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(PermissionEntry::parse)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns whether the document changed.
    pub fn add_permission(&mut self, category: PermissionCategory, entry: &PermissionEntry) -> bool {
        let text = entry.to_string();
        let arr = object_at(&mut self.0, "permissions")
            .and_then(|perms| array_at(perms, category.as_field()));
        let Some(arr) = arr else { return false };
        if arr.iter().any(|v| v.as_str() == Some(text.as_str())) {
            return false;
        }
        arr.push(Value::String(text));
        true
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_string(), value);
    }

    pub fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Claude Code deep-merges sandbox config across scopes, so contribute only
    /// this entry.
    pub fn allow_write(&mut self, path: &Path) {
        let text = path.to_string_lossy().into_owned();
        let Some(arr) = object_at(&mut self.0, "sandbox")
            .and_then(|sandbox| object_at(sandbox, "filesystem"))
            .and_then(|fs_map| array_at(fs_map, "allowWrite"))
        else {
            return;
        };
        if !arr.iter().any(|v| v.as_str() == Some(text.as_str())) {
            arr.push(Value::String(text));
        }
    }

    pub fn revoke_write(&mut self, path: &Path) {
        let text = path.to_string_lossy();
        let Some(arr) = self
            .0
            .get_mut("sandbox")
            .and_then(|v| v.get_mut("filesystem"))
            .and_then(|v| v.get_mut("allowWrite"))
            .and_then(Value::as_array_mut)
        else {
            return;
        };
        arr.retain(|v| v.as_str() != Some(text.as_ref()));
    }

    /// The per-repo file is symlinked into every worktree, so a torn write
    /// blanks permissions everywhere at once.
    pub async fn write_atomic(&self, path: &Path) -> AppResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let mut content = serde_json::to_string_pretty(&self.0)
            .map_err(|e| AppError::Other(format!("serializing settings.local.json: {e}")))?;
        content.push('\n');

        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, content).await?;
        fs::rename(&tmp, path).await?;
        Ok(())
    }
}

/// Replaces a non-object value.
fn object_at<'a>(map: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Map<String, Value>> {
    let entry = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut()
}

/// Replaces a non-array value.
fn array_at<'a>(map: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Vec<Value>> {
    let entry = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !entry.is_array() {
        *entry = Value::Array(Vec::new());
    }
    entry.as_array_mut()
}

#[cfg(test)]
mod tests {
    use super::*;
    use PermissionCategory::*;

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    const CORPUS: &[&str] = &[
        "Read(./src/**)",
        "Bash(yarn test:*)",
        "Bash(rm -rf /tmp/x)",
        "WebFetch(domain:github.com)",
        "Read(//Users/ryan/x/**)",
        "Read(~/Downloads/**)",
        "mcp__linear__get_issue",
        "Skill(see-data)",
        "Read()",
        "Edit(./a/b/c.ts)",
        "NoParens",
    ];

    #[test]
    fn parsing_round_trips_every_entry_shape() {
        for raw in CORPUS {
            assert_eq!(PermissionEntry::parse(raw).to_string(), *raw, "{raw}");
        }
    }

    #[test]
    fn scoping_to_a_repo_and_back_is_the_identity() {
        let repos = keys(&["frontend", "api"]);
        for raw in CORPUS {
            let entry = PermissionEntry::parse(raw);
            let scoped = entry.scoped_to_repo("frontend");
            match scoped.unscope(&repos) {
                Some((key, back)) => {
                    assert_eq!(key, "frontend", "{raw}");
                    assert_eq!(back, entry, "{raw}");
                }
                None => assert_eq!(
                    scoped, entry,
                    "{raw}: an entry with no ./ path must be left alone"
                ),
            }
        }
    }

    #[test]
    fn only_dot_slash_arguments_are_scoped() {
        let scope = |s: &str| PermissionEntry::parse(s).scoped_to_repo("frontend").to_string();
        assert_eq!(scope("Read(./src/**)"), "Read(./frontend/src/**)");
        assert_eq!(scope("Bash(yarn test:*)"), "Bash(yarn test:*)");
        assert_eq!(
            scope("WebFetch(domain:github.com)"),
            "WebFetch(domain:github.com)"
        );
        assert_eq!(scope("Read(//Users/ryan/x/**)"), "Read(//Users/ryan/x/**)");
        assert_eq!(scope("Read(~/Downloads/**)"), "Read(~/Downloads/**)");
        assert_eq!(scope("mcp__linear__get_issue"), "mcp__linear__get_issue");
        assert_eq!(scope("Skill(see-data)"), "Skill(see-data)");
    }

    #[test]
    fn unscope_recognizes_only_a_known_repo_prefix() {
        let repos = keys(&["api", "frontend"]);
        let un = |s: &str| PermissionEntry::parse(s).unscope(&repos);

        let (key, stripped) = un("Read(./api/src/foo.ts)").expect("matches");
        assert_eq!(key, "api");
        assert_eq!(stripped.to_string(), "Read(./src/foo.ts)");

        assert!(un("Read(./other/src/foo.ts)").is_none());
        assert!(un("Bash(rg:*)").is_none());
        assert!(un("mcp__linear__get_issue").is_none());
        assert!(un("Read(/abs/path)").is_none());
    }

    #[test]
    fn permissions_reads_each_category() {
        let mut doc = SettingsDoc::new();
        doc.add_permission(Allow, &PermissionEntry::parse("Read(./a)"));
        doc.add_permission(Deny, &PermissionEntry::parse("Bash(rm:*)"));

        assert_eq!(
            doc.permissions(Allow).iter().map(ToString::to_string).collect::<Vec<_>>(),
            vec!["Read(./a)"]
        );
        assert_eq!(
            doc.permissions(Deny).iter().map(ToString::to_string).collect::<Vec<_>>(),
            vec!["Bash(rm:*)"]
        );
        assert!(doc.permissions(Ask).is_empty());
    }

    #[test]
    fn add_permission_dedupes_and_reports_whether_it_changed() {
        let mut doc = SettingsDoc::new();
        let entry = PermissionEntry::parse("Read(./a)");
        assert!(doc.add_permission(Allow, &entry));
        assert!(!doc.add_permission(Allow, &entry));
        assert_eq!(doc.permissions(Allow).len(), 1);
    }

    #[test]
    fn allow_write_dedupes_and_preserves_siblings() {
        let mut doc = SettingsDoc::new();
        doc.set(
            "sandbox",
            serde_json::json!({ "network": { "allow": ["example.com"] } }),
        );
        let git_dir = Path::new("/data/repos/api/.git");
        doc.allow_write(git_dir);
        doc.allow_write(git_dir);

        let json = serde_json::to_value(&doc.0).unwrap();
        assert_eq!(
            json["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!(["/data/repos/api/.git"])
        );
        assert_eq!(
            json["sandbox"]["network"]["allow"],
            serde_json::json!(["example.com"]),
            "sibling sandbox config survives"
        );
    }

    #[test]
    fn revoke_write_removes_only_the_named_grant() {
        let mut doc = SettingsDoc::new();
        doc.allow_write(Path::new("/data/repos"));
        doc.allow_write(Path::new("/data/repos/api/.git"));
        doc.revoke_write(Path::new("/data/repos"));

        let json = serde_json::to_value(&doc.0).unwrap();
        assert_eq!(
            json["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!(["/data/repos/api/.git"])
        );
    }

    #[tokio::test]
    async fn unknown_keys_survive_a_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.local.json");
        std::fs::write(
            &path,
            r#"{"model":"opus","env":{"FOO":"bar"},"permissions":{"allow":["Read(./a)"]}}"#,
        )
        .unwrap();

        let mut doc = SettingsDoc::read(&path).await.unwrap();
        doc.add_permission(Allow, &PermissionEntry::parse("Read(./b)"));
        doc.write_atomic(&path).await.unwrap();

        let back: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back["model"], "opus");
        assert_eq!(back["env"]["FOO"], "bar");
        assert_eq!(
            back["permissions"]["allow"],
            serde_json::json!(["Read(./a)", "Read(./b)"])
        );
    }

    #[tokio::test]
    async fn a_missing_file_reads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = SettingsDoc::read(&tmp.path().join("nope.json")).await.unwrap();
        assert!(doc.is_empty());
    }

    #[tokio::test]
    async fn a_malformed_file_reads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.local.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(SettingsDoc::read(&path).await.unwrap().is_empty());

        std::fs::write(&path, "[1,2,3]").unwrap();
        assert!(SettingsDoc::read(&path).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn write_creates_parent_dirs_and_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("settings.local.json");
        let mut doc = SettingsDoc::new();
        doc.add_permission(Allow, &PermissionEntry::parse("Read(./a)"));
        doc.write_atomic(&path).await.unwrap();

        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with('\n'), "trailing newline preserved");
    }
}
