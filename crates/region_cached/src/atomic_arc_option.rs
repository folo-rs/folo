use std::{
    ptr,
    sync::{
        Arc,
        atomic::{self, AtomicPtr, AtomicUsize},
    },
};

/// Stores an Option<Arc> and provides atomic operations on the reference itself.
#[derive(Debug)]
pub struct AtomicArcOption<T> {
    inner: AtomicPtr<T>,

    /// Number of parties currently accessing `inner`.
    ///
    /// This is incremented before touching `inner` and decremented once the reference count of the
    /// `inner` (or what was in `inner` at time of read) has been decremented.
    ///
    /// Writers use this to understand when it is safe to decrement the reference count of the old
    /// value (when all readers have completed their work, which implies readers have incremented
    /// their reference counts so there is no danger of early release of resources).
    ///
    /// Writers will spin until this goes back down to equal `writers`, indicating that only writers
    /// are accessing the value. On every iteration, they need to fetch a new value for `writers`,
    /// of course, since more writers may arrive.
    ///
    /// If multiple writers are active, they will all wait together, each decrementing the ref
    /// count of whatever was the previous value for a the write they made.
    ///
    /// This is an approximation of a read-biased RwLock (readers have priority).
    activity_level: AtomicUsize,

    /// Number of writers that are tinkering with `inner`. Incremented before touching `inner` and
    /// decremented once the reference count of the old `inner` has been decremented.
    writers: AtomicUsize,
}

impl<T> AtomicArcOption<T> {
    pub fn new(value: Option<Arc<T>>) -> Self {
        let raw = match value {
            Some(value) => Arc::into_raw(value).cast_mut(),
            None => ptr::null_mut(),
        };

        Self {
            inner: AtomicPtr::new(raw),
            activity_level: AtomicUsize::new(0),
            writers: AtomicUsize::new(0),
        }
    }

    pub fn empty() -> Self {
        Self::new(None)
    }

    pub fn from_pointee(value: T) -> Self {
        Self::new(Some(Arc::new(value)))
    }

    pub fn load(&self) -> Option<Arc<T>> {
        // This guarantees that writers will not decrement the reference count
        // until we remove our activity from the activity level.
        self.activity_level.fetch_add(1, atomic::Ordering::AcqRel);

        let raw = self.inner.load(atomic::Ordering::Relaxed);

        if raw.is_null() {
            self.activity_level.fetch_sub(1, atomic::Ordering::AcqRel);
            return None;
        }

        // Note that we are not destroying the original - it is still in `inner` so this
        // load is actually a clone! We need to update the reference count to match.
        //
        // SAFETY: The ptr must have come from into_raw() - it did - and the T must match - it does.
        // The Arc must also be valid - it is, because we are still holding ownership via `inner`.
        unsafe {
            Arc::increment_strong_count(raw);
        }

        self.activity_level.fetch_sub(1, atomic::Ordering::AcqRel);

        // SAFETY: The ptr must have come from into_raw() - it did - and the T must match - it does.
        let arc = unsafe { Arc::from_raw(raw) };

        Some(arc)
    }

    pub fn store(&self, value: Option<Arc<T>>) {
        let raw = match value {
            Some(value) => Arc::into_raw(value).cast_mut(),
            None => ptr::null_mut(),
        };

        self.writers.fetch_add(1, atomic::Ordering::AcqRel);
        self.activity_level.fetch_add(1, atomic::Ordering::AcqRel);

        let previous = self.inner.swap(raw, atomic::Ordering::Relaxed);

        if previous.is_null() {
            self.writers.fetch_sub(1, atomic::Ordering::AcqRel);
            self.activity_level.fetch_sub(1, atomic::Ordering::AcqRel);
            return;
        }

        // We must wait until all readers have completed their work before we can decrement the
        // reference count of the old value, because part of the read operation is to increment
        // the reference count of the old value, so we need to ensure we do not destroy it first.
        loop {
            let writers = self.writers.load(atomic::Ordering::Acquire);
            let activity_level = self.activity_level.load(atomic::Ordering::Acquire);

            if activity_level > writers {
                // Not ready yet, spin spin spin spin spin spin spin spin spin spin spin spin.
                continue;
            }

            // Potentially ready (modulo concurrent trolling).
            // 1. We tentatively deregister ourselves as a writer.
            // 2. We deregister ourselves as an activity level participant.
            // If no activity increment has occurred meanwhile, we are good to release because we
            // know that no more readers (including competing writers pretending to be readers)
            // have appeared to potentially jeopardize our logic.
            //
            // Otherwise, we re-register as a writer and try again.

            self.writers.fetch_sub(1, atomic::Ordering::AcqRel);

            if self
                .activity_level
                .compare_exchange(
                    activity_level,
                    activity_level - 1,
                    atomic::Ordering::AcqRel,
                    atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                // Success!
                break;
            }

            // Failed, re-register as a writer and try again.
            self.writers.fetch_add(1, atomic::Ordering::AcqRel);
        }

        // SAFETY: The ptr must have come from into_raw() - it did - and the T must match - it does.
        // The Arc must also be valid - it is, because we are still holding ownership via `previous`.
        unsafe {
            Arc::decrement_strong_count(previous);
        }
    }
}

impl<T> Drop for AtomicArcOption<T> {
    fn drop(&mut self) {
        let raw = self.inner.load(atomic::Ordering::Relaxed);

        if raw.is_null() {
            return;
        }

        // SAFETY: The ptr must have come from into_raw() - it did - and the T must match - it does.
        // The Arc must also be valid - it is, because we are still holding ownership via `inner`.
        unsafe {
            Arc::decrement_strong_count(raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn smoke_test() {
        let atomic = AtomicArcOption::new(None);

        assert_eq!(atomic.load(), None);

        let value = Arc::new(42);
        atomic.store(Some(value.clone()));

        assert_eq!(atomic.load(), Some(value));

        atomic.store(None);
        assert_eq!(atomic.load(), None);
    }

    #[test]
    fn test_empty() {
        let atomic = AtomicArcOption::<u32>::empty();

        assert_eq!(atomic.load(), None);
    }

    #[test]
    fn test_drop() {
        let atomic = AtomicArcOption::new(Some(Arc::new(42)));

        // Ensure that the reference count is incremented on load.
        let loaded = atomic.load().unwrap();
        assert_eq!(Arc::strong_count(&loaded), 2);

        // The drop method should decrement the reference count.
        drop(atomic);
        assert_eq!(Arc::strong_count(&loaded), 1);
    }
}
