use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use infinity_pool::define_pooled_dyn_cast;

/// Type erasure trait for futures stored in [`FutureDeque`][crate::FutureDeque].
///
/// We cannot use `Future<Output = T>` directly with `define_pooled_dyn_cast!` because
/// `Future` uses an associated type, not a type parameter. This trait bridges the gap
/// by wrapping `Future::poll` behind a type-parameterized interface.
pub(crate) trait DequeFuture<T> {
    /// Polls the underlying future.
    fn poll_deque(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T>;
}

impl<T, F: Future<Output = T>> DequeFuture<T> for F {
    fn poll_deque(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        self.poll(cx)
    }
}

define_pooled_dyn_cast!(DequeFuture<T>);
