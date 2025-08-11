use std::cell::Cell;
use std::ops::Deref;
use std::sync::atomic::{self, AtomicBool};

/// A value that has two owners at the start, with the last remaining owner cleaning up the value.
#[derive(Debug)]
pub(crate) struct WithTwoOwners<T> {
    value: T,
    is_last_owner: AtomicBool,
}

impl<T> WithTwoOwners<T> {
    #[must_use]
    pub(crate) fn new(value: T) -> Self {
        Self {
            value,
            is_last_owner: AtomicBool::new(false),
        }
    }

    /// Releases ownership, returning a "should clean up" flag (set for the last owner).
    #[must_use]
    pub(crate) fn release_one(&self) -> bool {
        // Release because we are releasing the synchronization block of the `self`.
        if !self.is_last_owner.fetch_or(true, atomic::Ordering::Release) {
            // We were not the last owner - someone else will clean up.
            return false;
        }

        // We have detected that we were the last reference holder, so synchronize here.
        // We use an Acquire fence to ensure that we see all the writes from other threads
        // before the caller cleans up. This does nothing on x86 but writes may be delayed
        // on other architectures with weaker memory models.
        atomic::fence(atomic::Ordering::Acquire);

        true
    }
}

impl<T> Deref for WithTwoOwners<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

/// A value that has two owners at the start, with the last remaining owner cleaning up the value.
#[derive(Debug)]
pub(crate) struct LocalWithTwoOwners<T> {
    value: T,
    is_last_owner: Cell<bool>,
}

impl<T> LocalWithTwoOwners<T> {
    #[must_use]
    pub(crate) fn new(value: T) -> Self {
        Self {
            value,
            is_last_owner: Cell::new(false),
        }
    }

    /// Releases ownership, returning a "should clean up" flag (set for the last owner).
    #[must_use]
    pub(crate) fn release_one(&self) -> bool {
        if !self.is_last_owner.replace(true) {
            // We were not the last owner - someone else will clean up.
            return false;
        }

        true
    }
}

impl<T> Deref for LocalWithTwoOwners<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
