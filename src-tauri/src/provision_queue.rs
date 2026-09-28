//! One-at-a-time gate for provisioning. Concurrent setup scripts contend for
//! disk and CPU until they overrun `setup_timeout_secs`, which rolls the
//! workspace back. The semaphore is FIFO, so the first workspace asked for is
//! the first ready. In-memory only: queued jobs die with the app, like a
//! workspace caught mid-provision.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::job::JobTx;

const SLOTS: usize = 1;

#[derive(Clone)]
pub struct ProvisionQueue {
    slots: Arc<Semaphore>,
    /// A semaphore can't report its own queue length.
    waiting: Arc<AtomicUsize>,
}

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

    pub fn try_acquire(&self) -> Option<Slot> {
        self.slots.clone().try_acquire_owned().ok().map(Slot)
    }

    pub async fn acquire(&self) -> Slot {
        let _ticket = WaitTicket::new(&self.waiting);
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("provision queue is never closed");
        Slot(permit)
    }

    pub async fn acquire_announcing(&self, tx: &JobTx, repo: Option<&str>) -> Slot {
        match self.try_acquire() {
            Some(slot) => slot,
            None => {
                tx.status(self.wait_message(), repo);
                self.acquire().await
            }
        }
    }

    pub fn ahead(&self) -> usize {
        SLOTS.saturating_sub(self.slots.available_permits()) + self.waiting.load(Ordering::SeqCst)
    }

    pub fn wait_message(&self) -> String {
        match self.ahead() {
            0 | 1 => "waiting for another workspace to finish setting up".into(),
            n => format!("waiting for {n} workspaces ahead in the setup queue"),
        }
    }
}

/// A guard rather than paired add/sub so a future dropped mid-wait still
/// decrements.
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
