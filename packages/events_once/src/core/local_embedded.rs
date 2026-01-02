use std::any::type_name;
use std::fmt;
use std::mem::MaybeUninit;

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
/// // SAFETY: We promise that `task` lives longer than the endpoints.
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
pub struct EmbeddedLocalEvent<T: 'static> {
    pub(crate) inner: MaybeUninit<LocalEvent<T>>,
}

impl<T: 'static> EmbeddedLocalEvent<T> {
    /// Creates a new event container that an event can be placed into.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: MaybeUninit::uninit(),
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
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(EmbeddedLocalEvent<u32>: Send, Sync);
}
