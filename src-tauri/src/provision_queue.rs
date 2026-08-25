//! One-at-a-time gate for provisioning work.
//!
//! Cloning, `git worktree add` and — above all — a repo's setup script are the
//! most resource-hungry things Tethys does. Asking for five workspaces in a row
//! used to start five of them at once, all racing for the same disk, network
//! and CPU, and a `pnpm install` that takes two minutes alone can run past its
//! `setup_timeout_secs` when it's sharing the machine with four others. A
//! timeout there isn't a slow workspace, it's a failed one: provisioning rolls
//! the whole workspace back.
//!
//! So provisioning takes a slot here first, and there is exactly one. Jobs are
//! admitted in the order they asked (tokio's semaphore is FIFO), which is the
//! property that matters: the first workspace you asked for is the first one
//! you can start working in, rather than all five finishing together at the
//! end.
//!
//! The gate is in-memory and per-run. A queued job is a task parked on a
//! `.await`, not a persisted intent — quit Tethys with four workspaces waiting
//! and they're simply gone, which is the same thing that already happens to a
//! workspace caught mid-provision (`Store::load` prunes any draft that never
//! reached `Ready`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::job::JobTx;

/// How many provisioning jobs may run at once.
///
/// One. Not a tuning knob: the whole point is that the machine does one
/// install at a time, and anything else is a different feature (a budget)
/// wearing this one's clothes.
const SLOTS: usize = 1;

/// The queue itself. Cloneable and cheap — every clone shares one line.
#[derive(Clone)]
pub struct ProvisionQueue {
    slots: Arc<Semaphore>,
    /// Jobs parked in `acquire`, for the "N ahead of you" message. Tracked
    /// separately because a semaphore can't say how long its own queue is.
    waiting: Arc<AtomicUsize>,
}

/// A held slot. Provisioning runs for as long as this is alive; dropping it —
/// including by `?`, panic, or task cancellation — admits the next job.
pub struct Slot(#[allow(dead_code)] OwnedSemaphorePermit);

impl Default for ProvisionQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ProvisionQueue {
    pub fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(SLOTS)),
            waiting: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Take a slot only if one is free this instant. `None` means a job is
    /// already provisioning, and the caller is about to wait — which is the
    /// moment it wants to say so.
    pub fn try_acquire(&self) -> Option<Slot> {
        self.slots.clone().try_acquire_owned().ok().map(Slot)
    }

    /// Join the back of the queue and wait for a slot.
    pub async fn acquire(&self) -> Slot {
        let _ticket = WaitTicket::new(&self.waiting);
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            // Nothing closes the semaphore; it lives as long as the app does.
            .expect("provision queue is never closed");
        Slot(permit)
    }

    /// Take a slot, announcing the wait on `tx` if there is one. For callers
    /// with nothing to say beyond the log line — [`crate::provision`] spells
    /// the same thing out itself, because it also flips the row's status.
    pub async fn acquire_announcing(&self, tx: &JobTx, repo: Option<&str>) -> Slot {
        match self.try_acquire() {
            Some(slot) => slot,
            None => {
                tx.status(self.wait_message(), repo);
                self.acquire().await
            }
        }
    }

    /// How many jobs would run before one joining the queue right now: the
    /// slots in use plus everyone already waiting. A snapshot, and only ever
    /// used to write a sentence — by the time it's read it may be one lower.
    pub fn ahead(&self) -> usize {
        SLOTS.saturating_sub(self.slots.available_permits()) + self.waiting.load(Ordering::SeqCst)
    }

    /// The one sentence Tethys uses to explain a wait, wherever it happens.
    pub fn wait_message(&self) -> String {
        match self.ahead() {
            0 | 1 => "waiting for another workspace to finish setting up".into(),
            n => format!("waiting for {n} workspaces ahead in the setup queue"),
        }
    }
}

/// Counts one parked job for as long as it's parked — including when its
/// future is dropped mid-wait, which is why this is a guard and not a pair of
/// `fetch_add`/`fetch_sub` calls around the await.
struct WaitTicket(Arc<AtomicUsize>);

impl WaitTicket {
    fn new(waiting: &Arc<AtomicUsize>) -> Self {
        waiting.fetch_add(1, Ordering::SeqCst);
        Self(waiting.clone())
    }
}

impl Drop for WaitTicket {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    /// The whole contract: two jobs, never at the same time.
    #[tokio::test]
    async fn a_second_job_waits_for_the_first() {
        let queue = ProvisionQueue::new();
        let running = Arc::new(AtomicBool::new(false));
        let overlapped = Arc::new(AtomicBool::new(false));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let queue = queue.clone();
            let running = running.clone();
            let overlapped = overlapped.clone();
            handles.push(tokio::spawn(async move {
                let _slot = queue.acquire().await;
                if running.swap(true, Ordering::SeqCst) {
                    overlapped.store(true, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                running.store(false, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert!(
            !overlapped.load(Ordering::SeqCst),
            "two provisioning jobs ran at once"
        );
    }

    #[tokio::test]
    async fn an_idle_queue_admits_immediately() {
        let queue = ProvisionQueue::new();
        assert_eq!(queue.ahead(), 0);
        let slot = queue.try_acquire().expect("free slot");
        assert!(
            queue.try_acquire().is_none(),
            "the slot is taken until it's dropped"
        );
        drop(slot);
        assert!(queue.try_acquire().is_some());
    }

    /// The count behind the "N ahead" message: one running job plus each
    /// parked one, and back to zero once they've all been through.
    #[tokio::test]
    async fn waiting_jobs_are_counted_while_they_wait() {
        let queue = ProvisionQueue::new();
        let held = queue.try_acquire().expect("free slot");
        assert_eq!(queue.ahead(), 1, "the running job counts");

        let mut waiters = Vec::new();
        for _ in 0..2 {
            let queue = queue.clone();
            waiters.push(tokio::spawn(async move {
                let _slot = queue.acquire().await;
            }));
        }
        // Let both waiters reach their await.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(queue.ahead(), 3);
        assert_eq!(
            queue.wait_message(),
            "waiting for 3 workspaces ahead in the setup queue"
        );

        drop(held);
        for w in waiters {
            w.await.unwrap();
        }
        assert_eq!(queue.ahead(), 0);
    }

    /// A caller that gives up mid-wait (its task cancelled, its command
    /// dropped) must not leave a phantom in the count for the rest of the run.
    #[tokio::test]
    async fn abandoning_a_wait_leaves_no_phantom_in_the_queue() {
        let queue = ProvisionQueue::new();
        let held = queue.try_acquire().expect("free slot");

        let abandoned = {
            let queue = queue.clone();
            tokio::spawn(async move {
                let _slot = queue.acquire().await;
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(queue.ahead(), 2);

        abandoned.abort();
        let _ = abandoned.await;
        assert_eq!(queue.ahead(), 1, "only the running job is left");

        drop(held);
        assert_eq!(queue.ahead(), 0);
    }
}
