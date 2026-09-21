//! Cooperative cancellation for application work.
//!
//! Cancellation is cooperative: signalling a token asks work to stop, it does
//! not interrupt it. A durable side effect is therefore never rolled back by
//! cancelling a token; it uses an idempotency/intent record instead (see the
//! storage slices).

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// A cancellation scope that can be propagated to child tasks.
///
/// Cloning shares the same signal. Deriving a [`child`](Self::child) yields an
/// independent scope that the parent cancels but that does not cancel the
/// parent.
#[derive(Debug, Clone)]
pub struct CancellationScope {
    token: CancellationToken,
}

impl CancellationScope {
    /// Creates a new, uncancelled root scope.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Derives an independent child scope.
    ///
    /// Cancelling the child does not cancel the parent. Cancelling the parent
    /// cancels this and every other descendant once the parent's `cancel`
    /// returns.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
        }
    }

    /// Signals cancellation to this scope and its descendants.
    ///
    /// This is not atomically observed by every child while it is running, but
    /// after it returns all descendants are cancelled.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// Returns whether cancellation has been signalled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Completes when cancellation is signalled.
    ///
    /// This future is cancel safe, so it may be used in `select!` without
    /// risking a lost signal.
    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    /// Runs `future` until it completes or this scope is cancelled.
    ///
    /// Cancel safety is only as strong as `future`'s own. In a simultaneous
    /// race this favors `future` completion, so a side effect that must not
    /// repeat is guarded by durable state rather than by this helper.
    pub async fn run_until_cancelled<F: Future>(&self, future: F) -> Option<F::Output> {
        self.token.run_until_cancelled(future).await
    }

    /// Waits up to `grace` for cancellation to be observed.
    ///
    /// Returns `true` if the scope is cancelled within the bound and `false` if
    /// the bound elapses first. The bound exists so a drain never blocks
    /// shutdown indefinitely.
    pub async fn wait_for_cancellation(&self, grace: Duration) -> bool {
        tokio::time::timeout(grace, self.token.cancelled())
            .await
            .is_ok()
    }
}

impl Default for CancellationScope {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::CancellationScope;

    #[test]
    fn cancelling_the_parent_cancels_children() {
        let parent = CancellationScope::new();
        let child = parent.child();
        assert!(!child.is_cancelled());

        parent.cancel();
        assert!(parent.is_cancelled());
        assert!(child.is_cancelled());
    }

    #[test]
    fn cancelling_a_child_does_not_cancel_the_parent() {
        let parent = CancellationScope::new();
        let child = parent.child();

        child.cancel();
        assert!(child.is_cancelled());
        assert!(
            !parent.is_cancelled(),
            "a child must never cancel its parent",
        );
    }

    #[tokio::test]
    async fn cancelled_is_cancel_safe_when_cancelled_while_awaited() {
        let scope = CancellationScope::new();
        let waiter = {
            let scope = scope.clone();
            tokio::spawn(async move { scope.cancelled().await })
        };

        // Cancel concurrently with the await; the waiter must still resolve
        // rather than hanging or missing the signal.
        scope.cancel();

        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the waiter must resolve after cancellation")
            .expect("the waiter task must not panic");
    }

    #[tokio::test]
    async fn run_until_cancelled_returns_none_once_cancelled() {
        let scope = CancellationScope::new();
        scope.cancel();

        let result = scope
            .run_until_cancelled(std::future::pending::<()>())
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn run_until_cancelled_returns_the_value_when_it_completes_first() {
        let scope = CancellationScope::new();
        let result = scope.run_until_cancelled(async { 7_u8 }).await;
        assert_eq!(result, Some(7));
    }

    #[tokio::test]
    async fn wait_for_cancellation_honors_its_bound() {
        let scope = CancellationScope::new();
        let not_cancelled = scope.wait_for_cancellation(Duration::from_millis(20)).await;
        assert!(!not_cancelled, "an uncancelled scope must time out");

        scope.cancel();
        let cancelled = scope.wait_for_cancellation(Duration::from_millis(20)).await;
        assert!(cancelled, "a cancelled scope must report promptly");
    }
}
