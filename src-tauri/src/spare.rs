//! The spare: a workspace's worth of worktrees, one per enabled repo, built
//! ahead of time so a new workspace starts from installed dependencies rather
//! than from nothing. It sits detached in `<worktree_root>/.spare` until a
//! create claims it, then the next one is built behind it.
//!
//! Its build holds the setup queue like any other, so it never runs alongside
//! a real create. A create that arrives mid-build waits for it instead of
//! starting from scratch: the spare is already part way there.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::registry::{Registry, RepoRegistry};

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
    Ready(Built),
}

/// Where a spare is and what it holds. Kept rather than read off the registry,
/// which may have moved `worktree_root` since.
#[derive(Clone, Debug, PartialEq)]
struct Built {
    repos: Vec<String>,
    root: PathBuf,
}

pub struct Spares {
    slot: Mutex<Slot>,
    changed: Notify,
    /// The registry changed mid-build, so what's being built may already be
    /// the wrong thing.
    stale: AtomicBool,
    registry: Arc<Registry>,
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
    pub fn new(registry: Arc<Registry>, paths: Paths, queue: ProvisionQueue) -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new(Slot::Empty),
            changed: Notify::new(),
            stale: AtomicBool::new(false),
            registry,
            paths,
            queue,
        })
    }

    /// Keeps a spare left from the last run if it still matches the registry,
    /// and builds one otherwise.
    pub async fn start(self: &Arc<Self>) {
        let registry = self.registry.get();
        let Some(wanted) = registry.require().ok().and_then(wanted) else {
            return;
        };
        if let Some(found) = read_marker(&wanted.root).await {
            if found == wanted.repos && found.iter().all(|k| wanted.root.join(k).is_dir()) {
                info!(repos = ?found, "kept the spare from the last run");
                self.set(Slot::Ready(wanted));
                return;
            }
        }
        self.refill(None);
    }

    /// A spare for a different set of repos, or under a `worktree_root` that's
    /// been moved, is thrown away and rebuilt. Other changes need nothing: a
    /// create runs the setup script again and recomputes the rest.
    pub fn registry_changed(self: &Arc<Self>) {
        let registry = self.registry.get();
        let wanted = registry.require().ok().and_then(wanted);
        let mut slot = self.slot.lock().unwrap();
        match &*slot {
            Slot::Ready(built) if Some(built) == wanted.as_ref() => {}
            Slot::Ready(built) => {
                let old = built.root.clone();
                *slot = Slot::Empty;
                drop(slot);
                self.refill(Some(old));
            }
            Slot::Building => self.stale.store(true, Ordering::SeqCst),
            Slot::Empty => {
                drop(slot);
                self.refill(None);
            }
        }
    }

    /// Only a spare built for exactly `repos` is taken.
    pub fn try_take(self: &Arc<Self>, repos: &[String]) -> TryTake {
        let mut slot = self.slot.lock().unwrap();
        match &*slot {
            Slot::Ready(built) if built.repos == repos => {
                let built = built.clone();
                // The next one is on its way as soon as this lease is used, so
                // a create arriving now waits for that rather than starting
                // from scratch.
                *slot = Slot::Building;
                TryTake::Taken(SpareLease {
                    spares: self.clone(),
                    built: Some(built),
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
            self.refill(None);
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

    /// `discard_first` is an old spare somewhere the new one won't be built.
    fn refill(self: &Arc<Self>, discard_first: Option<PathBuf>) {
        self.set(Slot::Building);
        let this = self.clone();
        tokio::spawn(async move {
            if let Some(old) = discard_first {
                discard(&old, &this.paths).await;
            }
            let built = this.build().await;
            this.set(match built {
                Some(built) => Slot::Ready(built),
                None => Slot::Empty,
            });
            if this.stale.swap(false, Ordering::SeqCst) {
                this.registry_changed();
            }
        });
    }

    fn set(&self, next: Slot) {
        *self.slot.lock().unwrap() = next;
        self.changed.notify_waiters();
    }

    async fn build(&self) -> Option<Built> {
        let _slot = self.queue.acquire().await;
        // Read after the wait, which can be long, so it's current.
        let registry = self.registry.get();
        let reg = registry.require().ok()?;
        let repos = reg.enabled_repos();
        if repos.is_empty() {
            return None;
        }
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
        Some(Built { repos: keys, root })
    }
}

/// A taken spare. Dropped unused, it goes back as it was; once used, what's
/// left of it is thrown away and the next one built.
pub struct SpareLease {
    spares: Arc<Spares>,
    /// Only `None` once dropped.
    built: Option<Built>,
    used: bool,
}

impl SpareLease {
    /// Called before the first worktree is moved out.
    pub fn worktree(&mut self, repo_key: &str) -> PathBuf {
        let root = &self.built.as_ref().expect("held until drop").root;
        if !self.used {
            self.used = true;
            if let Err(e) = std::fs::remove_file(root.join(MARKER)) {
                warn!(error = %e, "could not unmark the spare");
            }
        }
        root.join(repo_key)
    }
}

impl Drop for SpareLease {
    fn drop(&mut self) {
        let built = self.built.take().expect("dropped once");
        if self.used {
            self.spares.refill(None);
        } else {
            self.spares.set(Slot::Ready(built));
            // Given back after an edit that made it the wrong spare.
            self.spares.registry_changed();
        }
    }
}

fn spare_root(reg: &RepoRegistry) -> PathBuf {
    reg.worktree_root.join(SPARE_DIR)
}

/// What a spare built now would be; `None` if there's nothing to build.
fn wanted(reg: &RepoRegistry) -> Option<Built> {
    let repos: Vec<String> = reg.enabled_repos().into_iter().map(|r| r.key).collect();
    (!repos.is_empty()).then(|| Built {
        repos,
        root: spare_root(reg),
    })
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
