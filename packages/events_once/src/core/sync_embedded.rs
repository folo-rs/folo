use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::panic::{RefUnwindSafe, UnwindSafe};

use crate::Event;

/// Container for an event that is embedded into a parent object.
///
/// An event can be placed into the container using [`Event::placed()`][1]. A single event
/// container may be reused for multiple events with non-overlapping lifetimes.
///
/// # Examples
///
/// ```
/// use events_once::{EmbeddedEvent, Event};
/// use pin_project::pin_project;
///
/// #[pin_project]
/// struct Task {
///     id: u64,
///
///     #[pin]
///     ready: EmbeddedEvent<()>,
/// }
///
/// # #[tokio::main]
/// # async fn main() {
/// let mut task = Box::pin(Task {
///     id: 42,
///     ready: EmbeddedEvent::new(),
/// });
///
/// // SAFETY: `Box::pin` keeps the task allocated and stationary for as long as we hold it,
/// // and the pinned projection carries that guarantee to the `ready` field, so the storage
/// // stays valid for writes and pinned while the endpoints exist. The field is a freshly
/// // constructed container, so no other event is using it. We access the storage only
/// // through the endpoints, both of which are consumed below before the task is used again.
/// let (ready_tx, ready_rx) = unsafe { Event::placed(task.as_mut().project().ready) };
///
/// ready_tx.send(());
/// ready_rx.await.unwrap();
///
/// println!("Task {} is ready!", task.id);
/// # }
/// ```
///
/// [1]: crate::Event::placed
pub struct EmbeddedEvent<T> {
    pub(crate) inner: UnsafeCell<MaybeUninit<Event<T>>>,
}

// The UnsafeCell causes auto-trait inference to mark EmbeddedEvent as !RefUnwindSafe, and the
// payload type inside it decides !UnwindSafe. Neither is a property of this container: it is
// storage only and offers no operation that reads or mutates the event or its payload. The
// event itself is reachable exclusively through the endpoints, and its state machine keeps
// that access panic-atomic, which is the same basis on which Event supplies RefUnwindSafe.
impl<T: Send + 'static> RefUnwindSafe for EmbeddedEvent<T> {}
impl<T: Send + 'static> UnwindSafe for EmbeddedEvent<T> {}

impl<T: Send + 'static> EmbeddedEvent<T> {
    /// Creates a new event container that an event can be placed into.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

impl<T: Send + 'static> Default for EmbeddedEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for EmbeddedEvent<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::Event;

    // `Box<dyn Send>` is the minimally qualified payload for this container: it satisfies the
    // `T: Send + 'static` bound and lacks every other auto trait, so these assertions verify
    // what the container supplies rather than what the payload happens to implement.
    // Preserving `Send` for such payloads is also a regression test for #142.
    assert_impl_all!(EmbeddedEvent<Box<dyn Send>>: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedEvent<Box<dyn Send>>: Sync);

    #[test]
    fn default_creates_usable_container() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::default());

        // SAFETY: `Box::pin` keeps the freshly created container allocated, valid for writes
        // and stationary for the rest of the test, and no other event has used this container.
        // The storage is accessed only through the endpoints below, both of which are dropped
        // before the container goes out of scope.
        let (sender, receiver) = unsafe { Event::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, std::task::Poll::Ready(Ok(42))));
    }
}
