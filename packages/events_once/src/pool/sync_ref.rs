use std::any::type_name;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::sync::Arc;
use std::{fmt, mem};

use infinity_pool::RawPooled;

use crate::{Event, EventPoolCore, EventRef};

pub(crate) struct PooledRef<T: Send> {
    core: Arc<EventPoolCore<T>>,
    event: RawPooled<UnsafeCell<MaybeUninit<Event<T>>>>,
}

impl<T: Send> PooledRef<T> {
    #[must_use]
    pub(crate) fn new(
        core: Arc<EventPoolCore<T>>,
        event: RawPooled<UnsafeCell<MaybeUninit<Event<T>>>>,
    ) -> Self {
        Self { core, event }
    }
}

impl<T: Send> Clone for PooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            event: self.event,
        }
    }
}

impl<T: Send> EventRef<T> for PooledRef<T> {
    fn release_event(&self) {
        let mut pool = self.core.pool.lock();

        // SAFETY: The event state machine guarantees that nothing references the event
        // once it signals the "you need to clean me up now". We hold the last reference.
        unsafe {
            pool.remove(self.event);
        }
    }
}

impl<T: Send> Deref for PooledRef<T> {
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event remains in the pool.
        let event_cell = unsafe { self.event.as_ref() };

        // SAFETY: We assert that the event has been initialized. This is always the case
        // by the time the PooledRef is created - the MaybeUninit wrapper is just there because
        // the items are delay-initialized after renting to avoid spurious memory copies.
        unsafe {
            mem::transmute::<&UnsafeCell<MaybeUninit<Event<T>>>, &UnsafeCell<Event<T>>>(event_cell)
        }
    }
}

impl<T: Send> fmt::Debug for PooledRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .field("event", &self.event)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(PooledRef<u32>: Send, Sync);
}
