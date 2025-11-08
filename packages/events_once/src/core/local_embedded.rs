use std::any::type_name;
use std::fmt;
use std::mem::MaybeUninit;

use crate::LocalEvent;

/// Container for a single-threaded event that is embedded into a parent object.
///
/// An event can be placed into the container using [`LocalEvent::placed()`][1]. A single event
/// container may be reused for multiple events with non-overlapping lifetimes.
///
/// [1]: crate::LocalEvent::placed
pub struct EmbeddedLocalEvent<T> {
    pub(crate) inner: MaybeUninit<LocalEvent<T>>,
}

impl<T> EmbeddedLocalEvent<T> {
    /// Creates a new event container that an event can be placed into.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: MaybeUninit::uninit(),
        }
    }
}

impl<T> Default for EmbeddedLocalEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for EmbeddedLocalEvent<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(EmbeddedLocalEvent<u32>: Send, Sync);
}
