use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::panic::RefUnwindSafe;

use crate::LocalEvent;

/// Container for a single-threaded event that is embedded into a parent object.
///
/// An event can be placed into the container using [`LocalEvent::placed()`][1]. A single event
/// container may be reused for multiple events with non-overlapping lifetimes.
///
/// # Examples
///
/// ```
/// use events_once::{EmbeddedLocalEvent, LocalEvent};
/// use pin_project::pin_project;
///
/// #[pin_project]
/// struct Task {
///     id: u64,
///
///     #[pin]
///     ready: EmbeddedLocalEvent<()>,
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let mut task = Box::pin(Task {
///     id: 42,
///     ready: EmbeddedLocalEvent::new(),
/// });
///
/// // SAFETY: The container was created right here as part of `task`, so no other event is using
/// // it, `task` stays alive and writable until both endpoints below are consumed, and `Box::pin`
/// // keeps the event at a stable address for that entire time.
/// let (ready_tx, ready_rx) = unsafe { LocalEvent::placed(task.as_mut().project().ready) };
///
/// ready_tx.send(());
/// ready_rx.await.unwrap();
///
/// println!("Task {} is ready!", task.id);
/// # }
/// ```
///
/// [1]: crate::LocalEvent::placed
pub struct EmbeddedLocalEvent<T> {
    pub(crate) inner: UnsafeCell<MaybeUninit<LocalEvent<T>>>,
}

// The `UnsafeCell` makes auto-trait inference mark the container as !RefUnwindSafe. The container
// only ever hands its storage to one event at a time, and the event it holds is unwind-safe
// regardless of the payload (see `LocalEvent`), so a caught panic cannot leave anything here that
// a later observer could see in an inconsistent state.
impl<T: 'static> RefUnwindSafe for EmbeddedLocalEvent<T> {}

impl<T: 'static> EmbeddedLocalEvent<T> {
    /// Creates a new event container that an event can be placed into.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

impl<T: 'static> Default for EmbeddedLocalEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for EmbeddedLocalEvent<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::rc::Rc;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::LocalEvent;

    assert_not_impl_any!(EmbeddedLocalEvent<u32>: Send, Sync);

    // The payload is one that is itself neither `UnwindSafe` nor `RefUnwindSafe`, so the
    // assertion can only pass if the container supplies both regardless of what it carries.
    assert_impl_all!(
        EmbeddedLocalEvent<Rc<RefCell<u32>>>: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn default_creates_usable_container() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::default());

        // SAFETY: The container was created right here, so no other event is using it, it stays
        // alive and writable until the endpoints below are gone, and `Box::pin` keeps the event
        // at a stable address for that entire time.
        let (sender, receiver) = unsafe { LocalEvent::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, std::task::Poll::Ready(Ok(42))));
    }
}
