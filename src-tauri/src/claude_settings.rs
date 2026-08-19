//! The `settings.local.json` document format, in one place.
//!
//! Claude Code's settings file is read at five sites and written at three
//! across `claude_local` and `pending_permissions`. Each site used to
//! re-implement the same three primitives — walk to `permissions.<field>`,
//! get-or-create the array, dedupe-push — and they had already drifted:
//! two logged a malformed file and one swallowed it, and the *same path* was
//! written atomically by one module and with a bare `fs::write` by the other,
//! despite being symlinked live into every worktree of a repo.
//!
//! The permission-entry grammar had drifted the same way: `rewrite_relative_path`
//! and `attribute` are declared inverses of each other but lived in different
//! modules, each with its own idea of what an entry looks like and no test
//! holding them together.

use std::path::Path;

use serde_json::{Map, Value};
use tokio::fs;
use tracing::warn;

use crate::error::{AppError, AppResult};

/// Which permission list an entry belongs to.
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

    /// Every category, in the order the workspace-root merge emits them.
    pub const ALL: [PermissionCategory; 3] = [
        PermissionCategory::Allow,
        PermissionCategory::Deny,
        PermissionCategory::Ask,
    ];
}

/// One permission entry, e.g. `Read(./src/**)`, `Bash(rm:*)`, `Skill(foo)`,
/// `mcp__server__tool`.
///
/// Parsed once here so scoping an entry to a repo and un-scoping it back are
/// provably inverse operations rather than two hand-rolled string walks in
/// different modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionEntry {
    /// Everything before the `(` — or the whole entry when it has no argument.
    tool: String,
    /// The text between the parens, if the entry has an argument at all.
    arg: Option<String>,
    /// Anything trailing the closing paren. Preserved rather than dropped.
    suffix: String,
}

impl PermissionEntry {
    /// Always succeeds: an entry we don't recognise is one with no argument,
    /// which is exactly how it should be treated.
    pub fn parse(entry: &str) -> Self {
        let no_arg = || Self {
            tool: entry.to_string(),
            arg: None,
            suffix: String::new(),
        };
        let (Some(open), Some(close)) = (entry.find('('), entry.rfind(')')) else {
            return no_arg();
        };
        // `Read()` carries no argument to rewrite.
        if close <= open + 1 {
            return no_arg();
        }
        Self {
            tool: entry[..open].to_string(),
            arg: Some(entry[open + 1..close].to_string()),
            suffix: entry[close + 1..].to_string(),
        }
    }

    /// Rewrite a worktree-relative path to be relative to the workspace root
    /// instead: `Read(./src/**)` → `Read(./api/src/**)`.
    ///
    /// Only `./`-prefixed arguments move. `Bash(...)`, `WebFetch(domain:...)`,
    /// absolute paths, `~/...` and argument-less entries are left alone.
    pub fn scoped_to_repo(&self, repo_key: &str) -> Self {
        let Some(rest) = self.arg.as_deref().and_then(|a| a.strip_prefix("./")) else {
            return self.clone();
        };
        Self {
            arg: Some(format!("./{repo_key}/{rest}")),
            ..self.clone()
        }
    }

    /// Inverse of [`PermissionEntry::scoped_to_repo`]: if the argument starts
    /// with `./<repo_key>/` for one of `repo_keys`, return that key and the
    /// entry with the prefix removed.
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

/// A `settings.local.json` document.
///
/// Wraps the raw object so unknown keys the user or Claude added survive every
/// edit — Tethys only ever contributes specific fields.
#[derive(Debug, Default, Clone)]
pub struct SettingsDoc(Map<String, Value>);

impl SettingsDoc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a settings file. A missing file, an unparseable one, or one whose
    /// root isn't an object all yield an empty document — but unlike the three
    /// hand-rolled readers this replaces, always with a warning naming the
    /// path.
    ///
    /// That silence mattered: `expected_from_per_repo` used to swallow a
    /// malformed per-repo file, which made its baseline incomplete and turned
    /// *every* entry in the combined file into a bogus Pending Permission,
    /// with no log line saying why.
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

    /// Like [`SettingsDoc::read`] but an unreadable file is an empty document
    /// rather than an error. For the callers that iterate many repos and must
    /// not abort the whole operation because one file is unreadable.
    pub async fn read_lossy(path: &Path) -> Self {
        Self::read(path).await.unwrap_or_default()
    }

    /// The entries in one permission category, in file order.
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

    /// Add an entry to a category if it isn't already there. Returns whether
    /// the document changed.
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

    /// Set a top-level key. Used for Tethys's own `_seededBy` marker.
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

    /// Ensure `sandbox.filesystem.allowWrite` contains `path`, creating the
    /// nested objects as needed and leaving sibling sandbox config alone —
    /// Claude Code deep-merges these across scopes, so we contribute only our
    /// own entry.
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

    /// Remove a grant a prior version seeded, leaving the surrounding
    /// structure untouched.
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

    /// Write via temp file + rename.
    ///
    /// Always atomic. The per-repo shared file is symlinked live into every
    /// worktree of that repo, so a torn write blanks permissions everywhere at
    /// once — yet one of the two writers of this exact path used a bare
    /// `fs::write`.
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

/// Get-or-create a nested object at `key`, replacing a non-object value.
fn object_at<'a>(map: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Map<String, Value>> {
    let entry = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut()
}

/// Get-or-create an array at `key`, replacing a non-array value.
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

    /// Every shape of entry Tethys sees in the wild.
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

    /// Parsing must be lossless — anything else silently corrupts a user's
    /// settings file the first time we touch it.
    #[test]
    fn parsing_round_trips_every_entry_shape() {
        for raw in CORPUS {
            assert_eq!(PermissionEntry::parse(raw).to_string(), *raw, "{raw}");
        }
    }

    /// The property the two halves of this grammar never had while they lived
    /// in different modules: scoping to a repo and un-scoping back is the
    /// identity.
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

        // `other` isn't one of this workspace's repos.
        assert!(un("Read(./other/src/foo.ts)").is_none());
        assert!(un("Bash(rg:*)").is_none());
        assert!(un("mcp__linear__get_issue").is_none());
        assert!(un("Read(/abs/path)").is_none());
    }

    // ── SettingsDoc ──────────────────────────────────────────────────────

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

    /// Tethys contributes specific fields; anything else in the file belongs
    /// to the user or to Claude and must survive every edit.
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

    /// A malformed file must not take the caller down with it — but unlike the
    /// reader this replaces in `expected_from_per_repo`, it warns rather than
    /// swallowing. That silence is what turned one broken per-repo file into a
    /// pile of bogus Pending Permissions.
    #[tokio::test]
    async fn a_malformed_file_reads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.local.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(SettingsDoc::read(&path).await.unwrap().is_empty());

        std::fs::write(&path, "[1,2,3]").unwrap();
        assert!(SettingsDoc::read(&path).await.unwrap().is_empty());
    }

    /// The per-repo file is symlinked live into every worktree of that repo,
    /// so a torn write blanks permissions everywhere at once.
    #[tokio::test]
    async fn writes_are_atomic_and_leave_no_temp_file() {
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
