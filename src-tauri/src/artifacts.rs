//! Artifacts: things a session produced that Tethys can show rather than
//! leave as text in the terminal.
//!
//! Two kinds. A **Diagram** is the mermaid source out of a fenced block in
//! the session's reply, read off the `Stop` hook's `last_assistant_message`.
//! A **Page** is an HTML file the session wrote inside the workspace, caught
//! from the `PostToolUse` hook for `Write`/`Edit`/`MultiEdit`. Both arrive
//! through hooks rather than by scraping xterm's buffer: the hook hands over
//! the exact source, unwrapped and complete, where the screen shows whatever
//! fraction of it is currently in the viewport.
//!
//! Artifacts belong to the workspace and are stored on it, so they ride along
//! in `state.json` and go when the workspace goes. The per-workspace cap is
//! what keeps that from becoming a graveyard: the design moves on, and the
//! twelve most recent things it produced is plenty of history.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tracing::{debug, warn};

use crate::state::WorkspaceId;
use crate::store::Store;

/// Most artifacts one workspace keeps; the oldest is evicted past this.
pub const CAP_PER_WORKSPACE: usize = 12;

/// Longest label a tab gets. Anything longer is cut with an ellipsis.
const LABEL_MAX_CHARS: usize = 18;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactKind {
    Diagram { source: String },
    Page { path: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: ArtifactKind,
    /// Bumped every time the same artifact is seen again (a page re-edited, a
    /// diagram re-emitted), so the UI can reload it in place.
    pub revision: u32,
}

/// Payload of `artifact:changed`. Carries the id of the artifact that just
/// arrived or was bumped, or `None` when one was dismissed.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactChanged {
    pub workspace_id: WorkspaceId,
    pub artifact_id: Option<String>,
}

/// The artifact operations, over the workspace they belong to.
pub struct ArtifactStore {
    store: Arc<Store>,
    app: AppHandle,
}

impl ArtifactStore {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        Self { store, app }
    }

    pub async fn list(&self, workspace_id: &str) -> Vec<Artifact> {
        self.store
            .with_workspace(workspace_id, |ws| ws.artifacts.clone())
            .await
            .unwrap_or_default()
    }

    /// Record every mermaid fence in a finished reply.
    pub async fn record_diagrams(&self, workspace_id: &str, message: &str) {
        let diagrams = extract_diagrams(message);
        if diagrams.is_empty() {
            debug!(workspace_id, "Stop with no mermaid fences");
            return;
        }
        for (label, source) in diagrams {
            self.record(workspace_id, label, ArtifactKind::Diagram { source })
                .await;
        }
    }

    /// Record an HTML file a tool just wrote, if it lives inside the workspace.
    pub async fn record_page(&self, workspace_id: &str, workspace_root: &Path, path: &Path) {
        if !crate::reconcile::is_under(workspace_root, path) {
            debug!(
                workspace_id,
                path = %path.display(),
                "ignoring HTML written outside the workspace"
            );
            return;
        }
        // Canonical, so `./tmp/x.html` from an `open` and the absolute path
        // from a `Write` are the same page. `is_under` just proved it resolves.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "page".to_string());
        self.record(workspace_id, label, ArtifactKind::Page { path })
            .await;
    }

    pub async fn dismiss(&self, workspace_id: &str, artifact_id: &str) {
        let result = self
            .store
            .update_workspace_quiet(workspace_id, |ws| {
                ws.artifacts.retain(|a| a.id != artifact_id);
                Ok(())
            })
            .await;
        match result {
            Ok(()) => self.emit(workspace_id, None),
            Err(e) => warn!(workspace_id, error = %e, "dismissing artifact"),
        }
    }

    async fn record(&self, workspace_id: &str, label: String, kind: ArtifactKind) {
        let result = self
            .store
            .update_workspace_quiet(workspace_id, |ws| {
                Ok(upsert(&mut ws.artifacts, label, kind))
            })
            .await;
        match result {
            Ok(id) => self.emit(workspace_id, Some(id)),
            Err(e) => warn!(workspace_id, error = %e, "recording artifact"),
        }
    }

    /// `artifact:changed` is the panel's own signal, separate from
    /// `workspace:changed`, so an arrival doesn't refetch every workspace and
    /// can carry which artifact to select.
    fn emit(&self, workspace_id: &str, artifact_id: Option<String>) {
        let _ = self.app.emit(
            "artifact:changed",
            ArtifactChanged {
                workspace_id: workspace_id.to_string(),
                artifact_id,
            },
        );
    }
}

/// Drop Pages whose file is gone. Runs at boot: a page is a path, and the
/// file behind it may have been cleaned up while Tethys wasn't looking.
pub fn prune_missing_pages(artifacts: &mut Vec<Artifact>) -> usize {
    let before = artifacts.len();
    artifacts.retain(|a| match &a.kind {
        ArtifactKind::Page { path } => path.exists(),
        ArtifactKind::Diagram { .. } => true,
    });
    before - artifacts.len()
}

/// Insert an artifact, or bump the one already there for the same thing.
///
/// "The same thing" is the same page path or the same diagram source: a page
/// Claude edits five times is one tab that reloads, not five. Either way the
/// artifact moves to the end — newest position — and the list is trimmed to
/// the cap from the front. Returns the id of the artifact touched.
fn upsert(list: &mut Vec<Artifact>, label: String, kind: ArtifactKind) -> String {
    let existing = list.iter().position(|a| a.kind == kind);
    let artifact = match existing {
        Some(i) => {
            let mut a = list.remove(i);
            a.revision += 1;
            a
        }
        None => Artifact {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            kind,
            revision: 0,
        },
    };
    let id = artifact.id.clone();
    list.push(artifact);
    if list.len() > CAP_PER_WORKSPACE {
        let excess = list.len() - CAP_PER_WORKSPACE;
        list.drain(..excess);
    }
    id
}

/// What a tool call tells us about a Page, if anything.
pub struct ToolCall<'a> {
    pub tool_name: Option<&'a str>,
    /// `tool_input.file_path` — set for the file tools.
    pub file_path: Option<&'a str>,
    /// `tool_input.command` — set for Bash.
    pub command: Option<&'a str>,
    /// Where a relative path in a Bash command resolves from.
    pub cwd: Option<&'a str>,
}

/// The Page a tool call produced, if it produced one: a file tool writing an
/// `.html` path, or a Bash command that `open`s one. The second is how the
/// `/show-me` skill ends every page — `open path/to/show-me-*.html` — and it
/// catches the file however it was written, which the file tools alone don't:
/// the very first page a session made for this was a heredoc.
pub fn page_written(call: &ToolCall<'_>) -> Option<PathBuf> {
    match call.tool_name? {
        "Write" | "Edit" | "MultiEdit" => html_path(PathBuf::from(call.file_path?)),
        "Bash" => {
            let opened = opened_paths(call.command?).into_iter().find_map(html_path)?;
            Some(match call.cwd {
                Some(cwd) if opened.is_relative() => Path::new(cwd).join(opened),
                _ => opened,
            })
        }
        _ => None,
    }
}

fn html_path(path: PathBuf) -> Option<PathBuf> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    (ext == "html" || ext == "htm").then_some(path)
}

/// Every non-flag argument to an `open` anywhere in a shell command line —
/// `mkdir -p x && open x/a.html; open -a Safari b.html` yields both files.
/// Quotes are honoured, nothing else is: no expansion, no substitution.
fn opened_paths(command: &str) -> Vec<PathBuf> {
    command
        .split(['\n', ';', '|', '&'])
        .filter_map(|segment| {
            let mut words = shell_words(segment).into_iter();
            (words.next().as_deref() == Some("open")).then(|| {
                words
                    .filter(|w| !w.starts_with('-'))
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            })
        })
        .flatten()
        .collect()
}

/// Whitespace split that keeps quoted spans together and drops the quotes.
fn shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Pull every ` ```mermaid ` fence out of a markdown reply, each with the
/// label its tab should wear.
pub fn extract_diagrams(message: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = message.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(fence) = open_fence(lines[i]) else {
            i += 1;
            continue;
        };
        let start = i + 1;
        let mut end = start;
        while end < lines.len() && !closes_fence(lines[end], fence) {
            end += 1;
        }
        let body = &lines[start..end.min(lines.len())];
        let source = dedent(body);
        if !source.trim().is_empty() {
            let label = diagram_label(&source, &lines[..i]);
            out.push((label, source));
        }
        i = end + 1;
    }
    out
}

/// A line that opens a mermaid fence: optional indent, three-or-more
/// backticks or tildes, then `mermaid` as the info string. Returns the fence
/// characters so the close can be matched against the same ones.
fn open_fence(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let marker = if trimmed.starts_with("```") {
        '`'
    } else if trimmed.starts_with("~~~") {
        '~'
    } else {
        return None;
    };
    let run = trimmed.chars().take_while(|&c| c == marker).count();
    let info = trimmed[run..].trim();
    let first_word = info.split_whitespace().next()?;
    first_word
        .eq_ignore_ascii_case("mermaid")
        .then_some(&trimmed[..run])
}

/// A closing fence: at least as many of the same characters, nothing else.
fn closes_fence(line: &str, fence: &str) -> bool {
    let trimmed = line.trim();
    let marker = fence.chars().next().unwrap_or('`');
    trimmed.len() >= fence.len() && trimmed.chars().all(|c| c == marker)
}

/// Strip the common leading whitespace — a fence inside a list item has every
/// body line indented under the bullet.
fn dedent(lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| if l.trim().is_empty() { "" } else { &l[indent..] })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The tab's label, from the best signal available: a `title` in the diagram
/// itself, then the heading or bold lead-in of the prose just above the
/// fence, then the diagram keyword. The fallbacks are never wrong, just dull.
fn diagram_label(source: &str, preceding: &[&str]) -> String {
    let label = title_directive(source)
        .or_else(|| lead_in(preceding))
        .or_else(|| diagram_keyword(source))
        .unwrap_or_else(|| "diagram".to_string());
    truncate(&label, LABEL_MAX_CHARS)
}

/// `title: X` (frontmatter) or `title X` (gantt, pie, …).
fn title_directive(source: &str) -> Option<String> {
    source.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("title")?;
        let rest = rest.strip_prefix(':').unwrap_or(rest);
        let title = rest.trim();
        (!title.is_empty() && rest.starts_with(|c: char| c.is_whitespace()))
            .then(|| title.to_string())
    })
}

/// The nearest non-blank line above the fence, if it reads as a title: a
/// markdown heading, a `**bold**` lead-in, or a short line on its own.
fn lead_in(preceding: &[&str]) -> Option<String> {
    let line = preceding.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty())?;
    if let Some(heading) = line.trim_start_matches('#').strip_prefix(' ') {
        if line.starts_with('#') {
            return Some(strip_punct(heading));
        }
    }
    if let Some(rest) = line.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            return Some(strip_punct(&rest[..end]));
        }
    }
    (line.chars().count() <= 40).then(|| strip_punct(line))
}

fn strip_punct(s: &str) -> String {
    s.trim()
        .trim_end_matches(['.', ':', '—', '-'])
        .trim()
        .to_string()
}

/// First word of the first line that isn't frontmatter or a comment —
/// `flowchart`, `sequenceDiagram`, `gantt`.
fn diagram_keyword(source: &str) -> Option<String> {
    let mut in_frontmatter = false;
    for line in source.lines() {
        let t = line.trim();
        if t == "---" {
            in_frontmatter = !in_frontmatter;
            continue;
        }
        if in_frontmatter || t.is_empty() || t.starts_with("%%") {
            continue;
        }
        return t.split_whitespace().next().map(|w| w.to_string());
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREENSHOT: &str = r#"**Today.** Every clip is its own round trip to the original recording.

```mermaid
flowchart LR
    A[audio.raw] --> B[prod_ingest]
```

The bottleneck is the red box.

**Proposed.** Build the parquet with `--inline-audio`.

```mermaid
flowchart LR
    A[audio.raw] --> F[parquet]
```
"#;

    #[test]
    fn extracts_every_fence_with_the_bold_lead_in_as_label() {
        let got = extract_diagrams(SCREENSHOT);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "Today");
        assert!(got[0].1.starts_with("flowchart LR\n"));
        assert_eq!(got[1].0, "Proposed");
    }

    #[test]
    fn ignores_other_fences_and_matches_the_fence_style() {
        let msg = "```rust\nfn x() {}\n```\n~~~mermaid\npie\n  \"a\" : 1\n~~~\n```\nplain\n```";
        let got = extract_diagrams(msg);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, "pie\n  \"a\" : 1");
    }

    #[test]
    fn dedents_a_fence_inside_a_list_item() {
        let msg = "- item\n\n  ```mermaid\n  graph TD\n    A --> B\n  ```\n";
        let got = extract_diagrams(msg);
        assert_eq!(got[0].1, "graph TD\n  A --> B");
    }

    #[test]
    fn an_unterminated_fence_runs_to_the_end() {
        let msg = "```mermaid\nflowchart TD\n  A --> B";
        let got = extract_diagrams(msg);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, "flowchart TD\n  A --> B");
    }

    #[test]
    fn label_prefers_title_then_heading_then_keyword() {
        let titled = "---\ntitle: Loader path\n---\nflowchart LR\n A-->B";
        assert_eq!(diagram_label(titled, &["**Today.**"]), "Loader path");
        let gantt = "gantt\n    title Release plan\n    section A";
        assert_eq!(diagram_label(gantt, &[]), "Release plan");
        assert_eq!(
            diagram_label("flowchart LR\n A-->B", &["## Data flow", ""]),
            "Data flow"
        );
        assert_eq!(
            diagram_label("sequenceDiagram\n A->>B: hi", &["Some long paragraph of prose that is far too long to be a title for anything."]),
            "sequenceDiagram"
        );
        assert_eq!(
            diagram_label("flowchart LR\n A-->B", &["A ridiculously long heading line"]),
            "A ridiculously lo…"
        );
    }

    #[test]
    fn same_diagram_bumps_instead_of_duplicating_and_moves_to_newest() {
        let mut list = Vec::new();
        let a = upsert(&mut list, "a".into(), ArtifactKind::Diagram { source: "A".into() });
        let b = upsert(&mut list, "b".into(), ArtifactKind::Diagram { source: "B".into() });
        let again = upsert(&mut list, "a2".into(), ArtifactKind::Diagram { source: "A".into() });
        assert_eq!(a, again);
        assert_ne!(a, b);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].id, a);
        assert_eq!(list[1].revision, 1);
        assert_eq!(list[1].label, "a", "a bump keeps the original label");
    }

    #[test]
    fn cap_evicts_the_oldest() {
        let mut list = Vec::new();
        for i in 0..(CAP_PER_WORKSPACE + 3) {
            upsert(&mut list, i.to_string(), ArtifactKind::Diagram { source: i.to_string() });
        }
        assert_eq!(list.len(), CAP_PER_WORKSPACE);
        assert_eq!(list[0].label, "3");
    }

    fn call<'a>(
        tool_name: &'a str,
        file_path: Option<&'a str>,
        command: Option<&'a str>,
    ) -> ToolCall<'a> {
        ToolCall {
            tool_name: Some(tool_name),
            file_path,
            command,
            cwd: Some("/w/repo"),
        }
    }

    #[test]
    fn boot_prune_drops_pages_whose_file_is_gone_and_keeps_diagrams() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("here.html");
        std::fs::write(&present, "<p/>").unwrap();
        let mut list = Vec::new();
        upsert(&mut list, "d".into(), ArtifactKind::Diagram { source: "graph TD".into() });
        upsert(&mut list, "here".into(), ArtifactKind::Page { path: present });
        upsert(&mut list, "gone".into(), ArtifactKind::Page { path: dir.path().join("gone.html") });
        assert_eq!(prune_missing_pages(&mut list), 1);
        assert_eq!(list.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["d", "here"]);
    }

    #[test]
    fn file_tools_count_only_for_html() {
        assert_eq!(
            page_written(&call("Write", Some("/w/show-me-x.html"), None)),
            Some(PathBuf::from("/w/show-me-x.html"))
        );
        assert!(page_written(&call("Edit", Some("/w/a.HTM"), None)).is_some());
        assert!(page_written(&call("Write", Some("/w/a.md"), None)).is_none());
        assert!(page_written(&call("Write", None, None)).is_none());
        assert!(page_written(&call("Read", Some("/w/a.html"), None)).is_none());
    }

    #[test]
    fn bash_open_of_an_html_file_counts_and_resolves_against_cwd() {
        let heredoc_then_open = "mkdir -p ./tmp && cat > ./tmp/x.html <<'HTML'\n<p>hi</p>\nHTML\nopen ./tmp/x.html";
        assert_eq!(
            page_written(&call("Bash", None, Some(heredoc_then_open))),
            Some(PathBuf::from("/w/repo/./tmp/x.html"))
        );
        assert_eq!(
            page_written(&call("Bash", None, Some("open -a \"Google Chrome\" /abs/p.html"))),
            Some(PathBuf::from("/abs/p.html"))
        );
        assert_eq!(
            page_written(&call("Bash", None, Some("open 'my page.html'"))),
            Some(PathBuf::from("/w/repo/my page.html"))
        );
        assert!(page_written(&call("Bash", None, Some("open https://example.com"))).is_none());
        assert!(page_written(&call("Bash", None, Some("cat > a.html <<EOF\nx\nEOF"))).is_none());
        assert!(page_written(&call("Bash", None, Some("echo open a.html"))).is_none());
    }
}
