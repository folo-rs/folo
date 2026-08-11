use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use multitude::dst::pointee;
use multitude::{Arena, coerce};

/// Type erasure trait for futures stored in a future deque.
///
/// `Future` cannot be erased directly because it carries its result as an associated type,
/// which leaves nothing for the deque to name its element type by. This trait restates
/// `Future::poll` behind a type parameter so that `dyn ErasedFuture<T>` identifies the
/// output type it produces.
///
/// The `pointee` attribute supplies the pointer-metadata implementation that
/// [`multitude::Box`] requires of its unsized targets. `pointee` is `ptr_meta`'s macro,
/// re-exported by `multitude`, and it emits `::ptr_meta::*` paths unless told otherwise;
/// the `crate` argument redirects those at `multitude`'s re-export so this package needs
/// no direct dependency on `ptr_meta`. Depending on `ptr_meta` directly would be worse: the
/// generated impls must target the exact `ptr_meta` instance `multitude` was built against,
/// so a version skew would surface as an unrelated-looking trait mismatch.
#[pointee(crate = ::multitude::dst)]
pub(crate) trait ErasedFuture<T> {
    /// Polls the underlying future.
    fn poll_erased(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T>;
}

impl<T, F: Future<Output = T>> ErasedFuture<T> for F {
    fn poll_erased(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        self.poll(cx)
    }
}

/// Owning handle to a type-erased future, as stored in a deque slot.
///
/// Both deque variants store this same type. The handle keeps its backing arena chunk alive
/// on its own, so it stays valid after the thread-local arena that produced it is gone.
pub(crate) type ErasedFutureHandle<T> = Pin<multitude::Box<dyn ErasedFuture<T>>>;

/// Moves `future` into the arena and erases its type.
pub(crate) fn erase<T, F: Future<Output = T> + 'static>(
    arena: &Arena,
    future: F,
) -> ErasedFutureHandle<T> {
    let handle = arena.alloc_box(future);

    multitude::Box::into_pin(multitude::Box::unsize(
        handle,
        coerce!(<T> dyn ErasedFuture<T>),
    ))
}
