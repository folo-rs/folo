use std::any::type_name;
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use infinity_pool::RawPooled;

use crate::{LocalEvent, LocalPoolCore, LocalRef};

pub(crate) struct PooledLocalRef<T> {
    core: Rc<LocalPoolCore<T>>,
    event: RawPooled<LocalEvent<T>>,
}

impl<T> PooledLocalRef<T> {
    #[must_use]
    pub(crate) fn new(core: Rc<LocalPoolCore<T>>, event: RawPooled<LocalEvent<T>>) -> Self {
        Self { core, event }
    }
}

impl<T> Clone for PooledLocalRef<T> {
    fn clone(&self) -> Self {
        Self {
            core: Rc::clone(&self.core),
            event: self.event,
        }
    }
}

impl<T> LocalRef<T> for PooledLocalRef<T> {
    fn release_event(&self) {
        let mut pool = self.core.pool.borrow_mut();

        // SAFETY: The event state machine guarantees that nothing references the event
        // once it signals the "you need to clean me up now". We hold the last reference.
        unsafe {
            pool.remove(self.event);
        }
    }
}

impl<T> Deref for PooledLocalRef<T> {
    type Target = LocalEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event remains in the pool.
        unsafe { self.event.as_ref() }
    }
}

impl<T> fmt::Debug for PooledLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .field("event", &self.event)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(PooledLocalRef<u32>: Send, Sync);
}
