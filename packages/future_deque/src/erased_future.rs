use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use infinity_pool::define_pooled_dyn_cast;

/// Type erasure trait for futures stored in a future deque.
///
/// We cannot use `Future<Output = T>` directly with `define_pooled_dyn_cast!` because
/// `Future` uses an associated type, not a type parameter. This trait bridges the gap
/// by wrapping `Future::poll` behind a type-parameterized interface.
pub(crate) trait ErasedFuture<T> {
    /// Polls the underlying future.
    fn poll_erased(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T>;
}

impl<T, F: Future<Output = T>> ErasedFuture<T> for F {
    fn poll_erased(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        self.poll(cx)
    }
}

define_pooled_dyn_cast!(ErasedFuture<T>);
