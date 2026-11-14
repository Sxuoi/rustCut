//! Cooperative cancellation of running FFmpeg jobs (EXP-09).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// A cancellation flag shared between the UI ("Cancel clip" / "Cancel all", EXP-09) and a
/// running job.
///
/// Cloning is cheap (one `Arc`) and every clone observes the same state. Cancelling is
/// permanent and idempotent: calling [`cancel`](Self::cancel) again has no further effect.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    /// A token that has not been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation and wakes every task waiting in [`cancelled`](Self::cancelled).
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Whether [`cancel`](Self::cancel) has been called on this token or any clone of it.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Completes once the token is cancelled (immediately if it already is).
    ///
    /// Cancel-safe: dropping the future loses nothing, so it can be used in `select!`.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.inner.notify.notified();
            let mut notified = std::pin::pin!(notified);
            // Register before checking the flag so a concurrent `cancel` cannot be missed.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn exp09_cancel_is_idempotent_and_shared_by_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled() && !clone.is_cancelled());
        clone.cancel();
        clone.cancel();
        assert!(token.is_cancelled() && clone.is_cancelled());
    }

    #[tokio::test]
    async fn exp09_cancelled_wakes_waiters_registered_before_and_after_cancel() {
        let token = CancelToken::new();
        let waiters: Vec<_> = (0..3)
            .map(|_| {
                let token = token.clone();
                tokio::spawn(async move { token.cancelled().await })
            })
            .collect();
        tokio::task::yield_now().await;
        token.cancel();
        for waiter in waiters {
            tokio::time::timeout(Duration::from_secs(5), waiter)
                .await
                .expect("waiter woken")
                .expect("waiter task ran");
        }
        tokio::time::timeout(Duration::from_secs(5), token.cancelled())
            .await
            .expect("an already-cancelled token completes immediately");
    }

    #[tokio::test]
    async fn uncancelled_token_stays_pending() {
        let token = CancelToken::new();
        let waited = tokio::time::timeout(Duration::from_millis(50), token.cancelled()).await;
        assert!(waited.is_err());
    }

    #[test]
    fn cancel_token_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CancelToken>();
    }
}
