//! The spare: a workspace's worth of worktrees, one per enabled repo, built
//! ahead of time so a new workspace starts from installed dependencies rather
//! than from nothing. It sits detached in `<worktree_root>/.spare` until a
//! create claims it, then the next one is built behind it.
//!
//! Its build holds the setup queue like any other, so it never runs alongside
//! a real create. A create that arrives mid-build waits for it instead of
//! starting from scratch: the spare is already part way there.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::git;
use crate::job::JobTx;
use crate::paths::Paths;
use crate::provision;
use crate::provision_queue::ProvisionQueue;
use crate::registry::{RegistryLoad, RepoRegistry};

/// Dotted so it can't collide with a workspace directory, which is named
/// after a branch.
pub const SPARE_DIR: &str = ".spare";

/// Written last, so a spare without one was interrupted mid-build. Removed
/// as soon as a create starts taking the spare apart.
const MARKER: &str = ".tethys-spare.json";

#[derive(Serialize, Deserialize)]
struct Marker {
    repos: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Slot {
    /// None built, none coming: the last build failed, or there's nothing to
    /// build.
    Empty,
    /// Being built, or being claimed with the next one to follow.
    Building,
    Ready(Vec<String>),
}

pub struct Spares {
    slot: Mutex<Slot>,
    changed: Notify,
    registry: Arc<RegistryLoad>,
    paths: Paths,
    queue: ProvisionQueue,
}

pub enum TryTake {
    Taken(SpareLease),
    /// One is on its way; `take` waits for it.
    Building,
    None,
}

impl Spares {
    pub fn new(registry: Arc<RegistryLoad>, paths: Paths, queue: ProvisionQueue) -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new(Slot::Empty),
            changed: Notify::new(),
            registry,
            paths,
            queue,
        })
    }

    /// Keeps a spare left from the last run if it still matches the registry,
    /// and builds one otherwise.
    pub async fn start(self: &Arc<Self>) {
        let Ok(reg) = self.registry.require() else {
            return;
        };
        let wanted = enabled_keys(reg);
        if wanted.is_empty() {
            return;
        }
        let root = spare_root(reg);
        if let Some(found) = read_marker(&root).await {
            if found == wanted && found.iter().all(|k| root.join(k).is_dir()) {
                info!(repos = ?found, "kept the spare from the last run");
                self.set(Slot::Ready(found));
                return;
            }
        }
        self.refill();
    }

    /// Only a spare built for exactly `repos` is taken.
    pub fn try_take(self: &Arc<Self>, repos: &[String]) -> TryTake {
        let mut slot = self.slot.lock().unwrap();
        match &*slot {
            Slot::Ready(keys) if keys == repos => {
                let keys = keys.clone();
                // The next one is on its way as soon as this lease is used, so
                // a create arriving now waits for that rather than starting
                // from scratch.
                *slot = Slot::Building;
                let root = self
                    .registry
                    .require()
                    .map(spare_root)
                    .expect("a spare is only built from a loaded registry");
                TryTake::Taken(SpareLease {
                    spares: self.clone(),
                    root,
                    repos: keys,
                    used: false,
                })
            }
            Slot::Building => TryTake::Building,
            Slot::Ready(_) | Slot::Empty => TryTake::None,
        }
    }

    pub async fn take(self: &Arc<Self>, repos: &[String]) -> Option<SpareLease> {
        loop {
            // Registered before the check, so a change between the two isn't
            // missed.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.try_take(repos) {
                TryTake::Taken(lease) => return Some(lease),
                TryTake::None => return None,
                TryTake::Building => changed.await,
            }
        }
    }

    /// For after a create that had no spare to use, so a failed build is
    /// retried rather than given up on.
    pub fn refill_if_empty(self: &Arc<Self>) {
        if *self.slot.lock().unwrap() == Slot::Empty {
            self.refill();
        }
    }

    /// Waits out a build in progress; true if a spare is then ready.
    #[cfg(test)]
    pub async fn settled(&self) -> bool {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match &*self.slot.lock().unwrap() {
                Slot::Building => {}
                Slot::Ready(_) => return true,
                Slot::Empty => return false,
            }
            changed.await;
        }
    }

    fn refill(self: &Arc<Self>) {
        self.set(Slot::Building);
        let this = self.clone();
        tokio::spawn(async move {
            let built = this.build().await;
            this.set(match built {
                Some(keys) => Slot::Ready(keys),
                None => Slot::Empty,
            });
        });
    }

    fn set(&self, next: Slot) {
        *self.slot.lock().unwrap() = next;
        self.changed.notify_waiters();
    }

    async fn build(&self) -> Option<Vec<String>> {
        let reg = self.registry.require().ok()?;
        let repos = reg.enabled_repos();
        if repos.is_empty() {
            return None;
        }
        let _slot = self.queue.acquire().await;
        let root = spare_root(reg);
        discard(&root, &self.paths).await;

        info!(repos = repos.len(), "building the spare");
        let started = std::time::Instant::now();
        let tx = JobTx::silent();
        for repo in &repos {
            let worktree = root.join(&repo.key);
            let built = async {
                let clone_path = provision::update_clone(repo, &self.paths, &tx).await?;
                git::worktree_add_detached(&clone_path, &worktree, &tx, &repo.key).await?;
                provision::fill_worktree(repo, &worktree, &self.paths, &tx).await?;
                AppResult::Ok(())
            }
            .await;
            if let Err(e) = built {
                warn!(repo = %repo.key, error = %e, "spare build failed; creates start from scratch");
                discard(&root, &self.paths).await;
                return None;
            }
        }

        let keys: Vec<String> = repos.into_iter().map(|r| r.key).collect();
        let marker = Marker { repos: keys.clone() };
        let written = serde_json::to_vec(&marker)
            .map_err(std::io::Error::other)
            .map(|json| std::fs::write(root.join(MARKER), json));
        if let Err(e) | Ok(Err(e)) = written {
            warn!(error = %e, "could not mark the spare built");
            discard(&root, &self.paths).await;
            return None;
        }
        info!(secs = started.elapsed().as_secs(), "spare ready");
        Some(keys)
    }
}

/// A taken spare. Dropped unused, it goes back as it was; once used, what's
/// left of it is thrown away and the next one built.
pub struct SpareLease {
    spares: Arc<Spares>,
    root: PathBuf,
    repos: Vec<String>,
    used: bool,
}

impl SpareLease {
    /// Called before the first worktree is moved out.
    pub fn worktree(&mut self, repo_key: &str) -> PathBuf {
        if !self.used {
            self.used = true;
            if let Err(e) = std::fs::remove_file(self.root.join(MARKER)) {
                warn!(error = %e, "could not unmark the spare");
            }
        }
        self.root.join(repo_key)
    }
}

impl Drop for SpareLease {
    fn drop(&mut self) {
        if self.used {
            self.spares.refill();
        } else {
            self.spares.set(Slot::Ready(std::mem::take(&mut self.repos)));
        }
    }
}

fn spare_root(reg: &RepoRegistry) -> PathBuf {
    reg.worktree_root.join(SPARE_DIR)
}

fn enabled_keys(reg: &RepoRegistry) -> Vec<String> {
    reg.enabled_repos().into_iter().map(|r| r.key).collect()
}

async fn read_marker(root: &Path) -> Option<Vec<String>> {
    let raw = tokio::fs::read(root.join(MARKER)).await.ok()?;
    serde_json::from_slice::<Marker>(&raw).ok().map(|m| m.repos)
}

/// Nothing in a spare has started anything outside it, so unlike purge there
/// is no teardown script to run.
async fn discard(root: &Path, paths: &Paths) {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return;
    };
    let tx = JobTx::silent();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let key = entry.file_name().to_string_lossy().into_owned();
        let clone_path = paths.repo_clone_path(&key);
        if clone_path.is_dir() {
            let _ = git::worktree_remove(&clone_path, &entry.path(), &tx, &key).await;
            git::worktree_prune_best_effort(&clone_path, &tx, &key).await;
        }
    }
    if let Err(e) = tokio::fs::remove_dir_all(root).await {
        warn!(path = %root.display(), error = %e, "could not remove the old spare");
    }
}
