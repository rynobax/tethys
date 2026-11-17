//! Things a session attached through `mcp__tethys__attach_artifact` for Tethys
//! to show: HTML **Pages** inside the workspace, and ummon **Reviews**.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use tracing::warn;

use crate::error::{AppError, AppResult};
use crate::state::WorkspaceId;
use crate::store::Store;
use crate::ui_socket::Events;

const CAP_PER_WORKSPACE: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactKind {
    Page { path: PathBuf },
    Review { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: ArtifactKind,
    /// Lets the UI reload a re-attached artifact in place.
    pub revision: u32,
}

/// `artifact_id` is `None` when one was removed.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactChanged {
    pub workspace_id: WorkspaceId,
    pub artifact_id: Option<String>,
}

pub struct ArtifactStore {
    store: Arc<Store>,
    events: Events,
}

impl ArtifactStore {
    pub fn new(events: Events, store: Arc<Store>) -> Self {
        Self { store, events }
    }

    pub async fn list(&self, workspace_id: &str) -> Vec<Artifact> {
        self.store
            .with_workspace(workspace_id, |ws| ws.artifacts.clone())
            .await
            .unwrap_or_default()
    }

    /// Attaching what's already attached bumps it to newest and reloads it.
    pub async fn attach(&self, workspace_id: &str, target: &str) -> AppResult<Artifact> {
        let root = self.root(workspace_id).await?;
        let (label, kind) = attachable(target, &root).map_err(AppError::Other)?;
        let artifact = self
            .store
            .update_workspace_quiet(workspace_id, |ws| {
                Ok(upsert(&mut ws.artifacts, label, kind))
            })
            .await?;
        self.emit(workspace_id, Some(artifact.id.clone()));
        Ok(artifact)
    }

    pub async fn detach(&self, workspace_id: &str, target: &str) -> AppResult<()> {
        let root = self.root(workspace_id).await?;
        let kind = resolve(target, &root);
        let removed = self
            .remove(workspace_id, |a| a.kind == kind)
            .await?;
        if !removed {
            return Err(AppError::Other(format!("nothing is attached as {target}")));
        }
        Ok(())
    }

    pub async fn dismiss(&self, workspace_id: &str, artifact_id: &str) {
        if let Err(e) = self.remove(workspace_id, |a| a.id == artifact_id).await {
            warn!(workspace_id, error = %e, "dismissing artifact");
        }
    }

    async fn remove(
        &self,
        workspace_id: &str,
        matches: impl Fn(&Artifact) -> bool,
    ) -> AppResult<bool> {
        let removed = self
            .store
            .update_workspace_quiet(workspace_id, |ws| {
                let before = ws.artifacts.len();
                ws.artifacts.retain(|a| !matches(a));
                Ok(ws.artifacts.len() != before)
            })
            .await?;
        if removed {
            self.emit(workspace_id, None);
        }
        Ok(removed)
    }

    async fn root(&self, workspace_id: &str) -> AppResult<PathBuf> {
        self.store
            .with_workspace(workspace_id, |ws| ws.root_buf())
            .await?
            .ok_or_else(|| AppError::Other("this workspace has no worktrees yet".into()))
    }

    fn emit(&self, workspace_id: &str, artifact_id: Option<String>) {
        self.events.emit(
            "artifact:changed",
            ArtifactChanged {
                workspace_id: workspace_id.to_string(),
                artifact_id,
            },
        );
    }
}

/// A URL is a Review and anything else a Page, its path taken from the
/// workspace root when relative.
fn resolve(target: &str, root: &Path) -> ArtifactKind {
    let target = target.trim();
    if target.starts_with("http://") || target.starts_with("https://") {
        return ArtifactKind::Review {
            url: target.to_string(),
        };
    }
    let path = root.join(target);
    // So `./x.html` and `/abs/x.html` are one page.
    let path = path.canonicalize().unwrap_or(path);
    ArtifactKind::Page { path }
}

/// Pages load through `tethys-file:`, which serves only files under the
/// workspace, and a Review's iframe is unsandboxed, so it must be local.
fn attachable(target: &str, root: &Path) -> Result<(String, ArtifactKind), String> {
    let kind = resolve(target, root);
    let label = match &kind {
        ArtifactKind::Review { url } if !is_local(url) => {
            return Err(format!("{url} isn't a localhost URL"));
        }
        ArtifactKind::Review { .. } => "review".to_string(),
        ArtifactKind::Page { path } if !path.is_file() => {
            return Err(format!("there's no file at {}", path.display()));
        }
        ArtifactKind::Page { path } if !crate::reconcile::is_under(root, path) => {
            return Err(format!("{} is outside this workspace", path.display()));
        }
        ArtifactKind::Page { path } => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "page".to_string()),
    };
    Ok((label, kind))
}

fn is_local(url: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"].iter().any(|host| {
        url.strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
    })
}

/// For `Workspace::artifacts`: an artifact of a kind since retired (diagrams)
/// is dropped rather than failing the whole of `state.json`.
pub fn known_only<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Artifact>, D::Error> {
    let raw = Vec::<serde_json::Value>::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

pub fn prune_missing_pages(artifacts: &mut Vec<Artifact>) -> usize {
    let before = artifacts.len();
    artifacts.retain(|a| match &a.kind {
        ArtifactKind::Page { path } => path.exists(),
        ArtifactKind::Review { .. } => true,
    });
    before - artifacts.len()
}

/// Same page path or URL bumps the existing artifact to newest.
fn upsert(list: &mut Vec<Artifact>, label: String, kind: ArtifactKind) -> Artifact {
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
    list.push(artifact.clone());
    if list.len() > CAP_PER_WORKSPACE {
        let excess = list.len() - CAP_PER_WORKSPACE;
        list.drain(..excess);
    }
    artifact
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(path: &str) -> ArtifactKind {
        ArtifactKind::Page { path: path.into() }
    }

    #[test]
    fn the_same_page_bumps_instead_of_duplicating_and_moves_to_newest() {
        let mut list = Vec::new();
        let a = upsert(&mut list, "a".into(), page("/w/a.html"));
        let b = upsert(&mut list, "b".into(), page("/w/b.html"));
        let again = upsert(&mut list, "a2".into(), page("/w/a.html"));
        assert_eq!(a.id, again.id);
        assert_ne!(a.id, b.id);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].id, a.id);
        assert_eq!(list[1].revision, 1);
        assert_eq!(list[1].label, "a", "a bump keeps the original label");
    }

    #[test]
    fn cap_evicts_the_oldest() {
        let mut list = Vec::new();
        for i in 0..(CAP_PER_WORKSPACE + 3) {
            upsert(&mut list, i.to_string(), page(&format!("/w/{i}.html")));
        }
        assert_eq!(list.len(), CAP_PER_WORKSPACE);
        assert_eq!(list[0].label, "3");
    }

    #[test]
    fn boot_prune_drops_pages_whose_file_is_gone_and_keeps_reviews() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("here.html");
        std::fs::write(&present, "<p/>").unwrap();
        let mut list = Vec::new();
        let review = ArtifactKind::Review {
            url: "http://localhost:7337/w/1".into(),
        };
        upsert(&mut list, "r".into(), review);
        upsert(&mut list, "here".into(), ArtifactKind::Page { path: present });
        upsert(&mut list, "gone".into(), ArtifactKind::Page { path: dir.path().join("gone.html") });
        assert_eq!(prune_missing_pages(&mut list), 1);
        assert_eq!(list.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["r", "here"]);
    }

    #[test]
    fn a_page_is_attachable_only_if_it_exists_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::write(root.join("repo/x.html"), "<p/>").unwrap();
        std::fs::write(dir.path().join("outside.html"), "<p/>").unwrap();

        let (label, kind) = attachable("repo/x.html", &root).unwrap();
        assert_eq!(label, "x.html");
        let abs = root.join("repo/x.html");
        assert_eq!(attachable(abs.to_str().unwrap(), &root).unwrap().1, kind);

        assert!(attachable("repo/missing.html", &root).is_err());
        assert!(attachable("../outside.html", &root).is_err());
    }

    #[test]
    fn only_a_localhost_url_is_attachable() {
        let root = Path::new("/w");
        let (label, kind) = attachable("http://localhost:7337/w/abc", root).unwrap();
        assert_eq!(label, "review");
        assert_eq!(
            kind,
            ArtifactKind::Review {
                url: "http://localhost:7337/w/abc".into()
            }
        );
        assert!(attachable("http://127.0.0.1:7337/w/abc", root).is_ok());
        assert!(attachable("https://evil.example/w/1", root).is_err());
        assert!(attachable("http://localhost.evil.example/", root).is_err());
    }

    #[test]
    fn a_stored_diagram_is_dropped_on_load() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "known_only")]
            artifacts: Vec<Artifact>,
        }
        let raw = r#"{"artifacts": [
            {"id": "1", "label": "d", "kind": "diagram", "source": "graph TD", "revision": 0},
            {"id": "2", "label": "p", "kind": "page", "path": "/w/p.html", "revision": 3}
        ]}"#;
        let held: Holder = serde_json::from_str(raw).unwrap();
        assert_eq!(held.artifacts.len(), 1);
        assert_eq!(held.artifacts[0].id, "2");
    }
}
