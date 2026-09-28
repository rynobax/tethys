//! Workspaces mid-create have directories but no state entry yet, so
//! `reconcile::scan` skips these ids instead of flagging them as orphans.

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

    pub fn insert(&self, id: String) -> InProgressGuard {
        self.inner.lock().unwrap().insert(id.clone());
        InProgressGuard {
            inner: self.inner.clone(),
            id,
        }
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
