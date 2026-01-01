use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;

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
/// // SAFETY: We promise that `task` lives longer than the endpoints.
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
pub struct EmbeddedEvent<T: Send + 'static> {
    pub(crate) inner: UnsafeCell<MaybeUninit<Event<T>>>,
}

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
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(EmbeddedEvent<u32>: Send);
    assert_not_impl_any!(EmbeddedEvent<u32>: Sync);
}
