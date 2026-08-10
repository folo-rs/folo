use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Type erasure trait for futures stored in a future deque.
///
/// `Future` cannot be erased directly because it carries its result as an associated type,
/// which leaves nothing for the deque to name its element type by. This trait restates
/// `Future::poll` behind a type parameter so that `dyn ErasedFuture<T>` identifies the
/// output type it produces.
///
/// The `pointee` attribute supplies the pointer-metadata implementation that
/// [`multitude::Box`] requires of its unsized targets. The `crate` argument redirects the
/// generated paths at `multitude`'s re-export of that vocabulary, so this package needs no
/// direct dependency on the underlying `ptr_meta` crate.
#[multitude::dst::pointee(crate = ::multitude::dst)]
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
    arena: &multitude::Arena,
    future: F,
) -> ErasedFutureHandle<T> {
    let handle = arena.alloc_box(future);

    multitude::Box::into_pin(multitude::Box::unsize(
        handle,
        multitude::coerce!(<T> dyn ErasedFuture<T>),
    ))
}
