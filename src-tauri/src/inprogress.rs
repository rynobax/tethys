//! Workspace directory names claimed by a create that hasn't finished. They
//! may not exist on disk yet, and aren't in state yet, so both
//! `branch_name::reserve` and `reconcile::scan` have to consult this set.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct InProgressWorkspaces {
    inner: Arc<Mutex<HashSet<String>>>,
}

impl InProgressWorkspaces {
    pub fn new() -> Self {
        Self::default()
    }

    /// `choose` runs under the lock and names the entry to claim, so two
    /// callers can't both pick the same free name.
    pub fn claim<T, E>(
        &self,
        choose: impl FnOnce(&HashSet<String>) -> Result<(String, T), E>,
    ) -> Result<(InProgressGuard, T), E> {
        let mut set = self.inner.lock().unwrap();
        let (id, value) = choose(&set)?;
        set.insert(id.clone());
        let guard = InProgressGuard {
            inner: self.inner.clone(),
            id,
        };
        Ok((guard, value))
    }

    pub fn snapshot(&self) -> HashSet<String> {
        self.inner.lock().unwrap().clone()
    }
}

pub struct InProgressGuard {
    inner: Arc<Mutex<HashSet<String>>>,
    id: String,
}

impl Drop for InProgressGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.inner.lock() {
            set.remove(&self.id);
        }
    }
}
