//! Writes an edit from the settings UI back into `repos.toml`. Only what
//! changed is touched, through `toml_edit`, so comments, ordering and the
//! formatting of everything else survive: the file stays one a person can edit
//! by hand.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table};

use crate::error::{AppError, AppResult};
use crate::registry::{starter_template, Repo, RepoRegistry};

/// The whole registry as the settings UI has it.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryEdit {
    pub worktree_root: PathBuf,
    pub repos: Vec<Repo>,
    /// Empty or `None` for Tethys's built-in text.
    #[serde(default)]
    pub workspace_doc_body: Option<String>,
}

/// `existing` is the file as it is now, empty if there isn't one.
pub fn apply(existing: &str, edit: &RegistryEdit) -> AppResult<String> {
    let base = if existing.trim().is_empty() {
        starter_template()
    } else {
        existing
    };
    let mut doc: DocumentMut = base
        .parse()
        .map_err(|e| AppError::Other(format!("repos.toml doesn't parse, so fix it by hand first: {e}")))?;
    // What the file says now, so an unchanged field is left exactly as written.
    let old: Option<RepoRegistry> = toml::from_str(base).ok();

    if old.as_ref().map(|o| &o.worktree_root) != Some(&edit.worktree_root) {
        doc["worktree_root"] = value(edit.worktree_root.to_string_lossy().as_ref());
    }

    apply_workspace_doc(&mut doc, old.as_ref(), edit.workspace_doc_body.as_deref());

    let old_repos: &[Repo] = old.as_ref().map(|o| o.repos.as_slice()).unwrap_or(&[]);
    apply_repos(&mut doc, old_repos, &edit.repos)?;

    Ok(doc.to_string())
}

fn apply_workspace_doc(doc: &mut DocumentMut, old: Option<&RepoRegistry>, body: Option<&str>) {
    let body = body.map(str::trim_end).filter(|b| !b.trim().is_empty());
    let old_body = old
        .and_then(|o| o.workspace_doc.as_ref())
        .map(|d| d.body.trim_end())
        .filter(|b| !b.trim().is_empty());
    if body == old_body {
        return;
    }
    match body {
        None => {
            doc.remove("workspace_doc");
        }
        Some(body) => {
            if !doc.contains_table("workspace_doc") {
                doc["workspace_doc"] = Item::Table(Table::new());
            }
            doc["workspace_doc"]["body"] = value(format!("{body}\n"));
        }
    }
}

fn apply_repos(doc: &mut DocumentMut, old: &[Repo], repos: &[Repo]) -> AppResult<()> {
    if !doc.contains_array_of_tables("repo") {
        if repos.is_empty() {
            return Ok(());
        }
        doc["repo"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let tables = doc["repo"]
        .as_array_of_tables_mut()
        .ok_or_else(|| AppError::Other("repos.toml's `repo` isn't a list of [[repo]] tables".into()))?;

    let key_of = |t: &Table| t.get("key").and_then(|k| k.as_str()).map(String::from);

    // Back to front, so removing doesn't shift what's left to check.
    for i in (0..tables.len()).rev() {
        let gone = tables
            .get(i)
            .and_then(key_of)
            .is_none_or(|key| !repos.iter().any(|r| r.key == key));
        if gone {
            tables.remove(i);
        }
    }

    for repo in repos {
        let existing = tables
            .iter_mut()
            .find(|t| key_of(t).as_deref() == Some(repo.key.as_str()));
        match existing {
            Some(table) => {
                let before = old.iter().find(|r| r.key == repo.key);
                write_repo(table, before, repo);
            }
            None => {
                let mut table = Table::new();
                table["key"] = value(repo.key.as_str());
                write_repo(&mut table, None, repo);
                tables.push(table);
            }
        }
    }
    Ok(())
}

/// Writes each field of `repo` that differs from `before`. A field at its
/// default is removed rather than written out.
fn write_repo(table: &mut Table, before: Option<&Repo>, repo: &Repo) {
    let changed = |f: fn(&Repo) -> Item| before.map(f).map(|b| b.to_string()) != Some(f(repo).to_string());

    if changed(|r| value(r.remote_url.as_str())) {
        table["remote_url"] = value(repo.remote_url.as_str());
    }
    if changed(|r| value(r.enabled)) {
        set(table, "enabled", (!repo.enabled).then(|| value(false)));
    }
    type Field = fn(&Repo) -> &Option<String>;
    let optional_strings: [(&str, Field); 4] = [
        ("default_branch", |r| &r.default_branch),
        ("default_setup_script", |r| &r.default_setup_script),
        ("teardown_script", |r| &r.teardown_script),
        ("claude_notes", |r| &r.claude_notes),
    ];
    for (name, field) in optional_strings {
        let now = non_empty(field(repo));
        // Trailing whitespace aside: a `"""` string in the file ends in a
        // newline the form trims off.
        let same = before.is_some_and(|b| {
            non_empty(field(b)).map(str::trim_end) == now.map(str::trim_end)
        });
        if !same {
            set(table, name, now.map(value));
        }
    }
    if before.map(|b| b.setup_timeout_secs) != Some(repo.setup_timeout_secs) {
        set(
            table,
            "setup_timeout_secs",
            repo.setup_timeout_secs.map(|s| value(s as i64)),
        );
    }
    if before.map(|b| &b.copy_files) != Some(&repo.copy_files) {
        set(table, "copy_files", string_array(&repo.copy_files));
    }
    if before.map(|b| &b.env) != Some(&repo.env) {
        set(table, "env", inline_table(&repo.env));
    }
}

fn set(table: &mut Table, name: &str, item: Option<Item>) {
    match item {
        Some(item) => table[name] = item,
        None => {
            table.remove(name);
        }
    }
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.trim().is_empty())
}

fn string_array(items: &[String]) -> Option<Item> {
    let items: Vec<&str> = items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    (!items.is_empty()).then(|| value(items.into_iter().collect::<Array>()))
}

fn inline_table(env: &BTreeMap<String, String>) -> Option<Item> {
    if env.is_empty() {
        return None;
    }
    let mut table = InlineTable::new();
    for (k, v) in env {
        table.insert(k, v.as_str().into());
    }
    Some(value(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"#:schema ./repos.schema.json
# my notes up top
worktree_root = "/tmp/wt"

[[repo]]
key = "web"
remote_url = "git@github.com:me/web.git"
# install everything
default_setup_script = "pnpm install"
copy_files = [".env"]

[[repo]]
key = "api"
remote_url = "git@github.com:me/api.git"
env = { COMPOSE_PROJECT_NAME = "{repo}-{workspace}" }
"#;

    fn current() -> RegistryEdit {
        let reg: RepoRegistry = toml::from_str(FILE).unwrap();
        RegistryEdit {
            worktree_root: reg.worktree_root,
            repos: reg.repos,
            workspace_doc_body: None,
        }
    }

    fn reparse(s: &str) -> RepoRegistry {
        toml::from_str(s).unwrap_or_else(|e| panic!("{e}\n{s}"))
    }

    #[test]
    fn saving_without_changes_leaves_the_file_as_it_was() {
        assert_eq!(apply(FILE, &current()).unwrap(), FILE);
    }

    #[test]
    fn a_changed_field_is_rewritten_and_the_comments_kept() {
        let mut edit = current();
        edit.repos[0].default_setup_script = Some("pnpm install --frozen-lockfile".into());
        let out = apply(FILE, &edit).unwrap();
        assert!(out.contains("# my notes up top"), "{out}");
        assert!(out.contains("# install everything"), "{out}");
        assert_eq!(
            reparse(&out).repos[0].default_setup_script.as_deref(),
            Some("pnpm install --frozen-lockfile")
        );
    }

    #[test]
    fn disabling_writes_enabled_false_and_enabling_removes_it() {
        let mut edit = current();
        edit.repos[1].enabled = false;
        let disabled = apply(FILE, &edit).unwrap();
        assert!(disabled.contains("enabled = false"), "{disabled}");
        assert!(!reparse(&disabled).repos[1].enabled);

        edit.repos[1].enabled = true;
        let enabled = apply(&disabled, &edit).unwrap();
        assert!(!enabled.contains("enabled"), "{enabled}");
    }

    #[test]
    fn repos_are_added_and_removed() {
        let mut edit = current();
        edit.repos.remove(0);
        let mut new: Repo = toml::from_str(
            r#"
            key = "docs"
            remote_url = "git@github.com:me/docs.git"
            "#,
        )
        .unwrap();
        new.copy_files = vec![".env".into(), "  ".into()];
        edit.repos.push(new);
        let out = apply(FILE, &edit).unwrap();
        let keys: Vec<String> = reparse(&out).repos.into_iter().map(|r| r.key).collect();
        assert_eq!(keys, ["api", "docs"]);
        assert_eq!(reparse(&out).repos[1].copy_files, [".env"]);
        assert!(!out.contains("install everything"), "the removed repo's comment goes with it");
    }

    #[test]
    fn cleared_fields_are_removed_rather_than_left_empty() {
        let mut edit = current();
        edit.repos[0].default_setup_script = Some("  ".into());
        edit.repos[0].copy_files.clear();
        edit.repos[1].env.clear();
        let out = apply(FILE, &edit).unwrap();
        assert!(!out.contains("default_setup_script"), "{out}");
        assert!(!out.contains("copy_files"), "{out}");
        assert!(!out.contains("env ="), "{out}");
    }

    #[test]
    fn multi_line_text_is_written_readably() {
        let mut edit = current();
        edit.repos[0].claude_notes = Some("Line one.\nLine \"two\".\n".into());
        edit.workspace_doc_body = Some("# Workspace {branch}\n\nHello.".into());
        let out = apply(FILE, &edit).unwrap();
        let reg = reparse(&out);
        assert_eq!(reg.repos[0].claude_notes.as_deref(), Some("Line one.\nLine \"two\".\n"));
        assert_eq!(reg.workspace_doc.unwrap().body, "# Workspace {branch}\n\nHello.\n");
        assert!(!out.contains("\\n"), "newlines escaped onto one line:\n{out}");
    }

    #[test]
    fn clearing_the_workspace_doc_returns_to_the_built_in_text() {
        let mut edit = current();
        edit.workspace_doc_body = Some("Custom".into());
        let custom = apply(FILE, &edit).unwrap();
        edit.workspace_doc_body = Some(String::new());
        let cleared = apply(&custom, &edit).unwrap();
        assert!(reparse(&cleared).workspace_doc.is_none(), "{cleared}");
    }

    #[test]
    fn a_missing_file_starts_from_the_commented_template() {
        let mut edit = current();
        edit.repos.truncate(1);
        let out = apply("", &edit).unwrap();
        assert!(out.contains("# Tethys repo registry"), "{out}");
        let reg = reparse(&out);
        assert_eq!(reg.worktree_root, PathBuf::from("/tmp/wt"));
        assert_eq!(reg.repos.len(), 1);
    }
}
