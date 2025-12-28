use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU8, Ordering};

use parking_lot_core::{DEFAULT_PARK_TOKEN, DEFAULT_UNPARK_TOKEN, park, unpark_one};

// 0: Unlocked
// 1: Locked, no waiters
// 2: Locked, waiters (or potential waiters)
const UNLOCKED: u8 = 0;
const LOCKED: u8 = 1;
const CONTENDED: u8 = 2;

/// A mutex that never spins.
///
/// This is useful for synchronizing threads that are guaranteed to run on the same
/// processor, where spinning would just waste CPU cycles without any chance of
/// the lock holder making progress.
pub(crate) struct SpinFreeMutex<T> {
    state: AtomicU8,
    data: UnsafeCell<T>,
}

/// SAFETY: `SpinFreeMutex` is Send if T is Send, because it provides exclusive access to T.
unsafe impl<T: Send> Send for SpinFreeMutex<T> {}
/// SAFETY: `SpinFreeMutex` is Sync if T is Send, because it allows multiple threads to access T
/// (via lock), but only one at a time.
unsafe impl<T: Send> Sync for SpinFreeMutex<T> {}

impl<T> SpinFreeMutex<T> {
    pub(crate) const fn new(data: T) -> Self {
        Self {
            state: AtomicU8::new(UNLOCKED),
            data: UnsafeCell::new(data),
        }
    }

    #[inline]
    pub(crate) fn lock(&self) -> SpinFreeMutexGuard<'_, T> {
        if self
            .state
            .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.lock_slow();
        }
        SpinFreeMutexGuard { lock: self }
    }

    #[cold]
    fn lock_slow(&self) {
        loop {
            let state = self.state.load(Ordering::Relaxed);

            // If unlocked, try to acquire.
            // If we are in the slow path, we generally assume there might be contention,
            // but if we see UNLOCKED, we can try to grab it as LOCKED first.
            // If we fail, we'll loop and likely see LOCKED/CONTENDED.
            if state == UNLOCKED {
                if self
                    .state
                    .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
                continue;
            }

            // If locked, try to mark as contended.
            if state == LOCKED
                && self
                    .state
                    .compare_exchange(LOCKED, CONTENDED, Ordering::Relaxed, Ordering::Relaxed)
                    .is_err()
            {
                continue;
            }

            // Park.
            let addr = self.state.as_ptr() as usize;
            let validate = || {
                // If the state is no longer CONTENDED (or LOCKED), we should abort parking.
                // Actually, we only care if it is UNLOCKED.
                // If it is LOCKED, we should still park (because we set it to CONTENDED above,
                // but if someone else unlocked and locked again as LOCKED, we should fix it to CONTENDED?
                // No, if we set it to CONTENDED, it stays CONTENDED until unlock.
                // So if it is not CONTENDED, it must be UNLOCKED (because we just set it to CONTENDED).
                // Wait, if we set it to CONTENDED, and then someone unlocks, it becomes UNLOCKED.
                // So if it is UNLOCKED, we don't park.
                // SAFETY: The address is derived from a reference to the atomic state, so it is valid.
                let state = unsafe { &*(addr as *const AtomicU8) };
                state.load(Ordering::Relaxed) != UNLOCKED
            };

            // SAFETY: We are parking on the address of the state atomic. The validation function checks
            // if the state is still not UNLOCKED.
            unsafe {
                park(addr, validate, || {}, |_, _| {}, DEFAULT_PARK_TOKEN, None);
            }

            // We woke up. Try to acquire.
            // Since we woke up from contention, we assume there might be others,
            // so we try to acquire as CONTENDED.
            // This ensures that when we unlock, we wake up the next person.
            if self
                .state
                .compare_exchange(UNLOCKED, CONTENDED, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    #[inline]
    pub(crate) fn unlock(&self) {
        // Fast path: if LOCKED, just unlock.
        if self
            .state
            .compare_exchange(LOCKED, UNLOCKED, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }

        // Slow path: CONTENDED.
        self.unlock_slow();
    }

    #[cold]
    fn unlock_slow(&self) {
        self.state.store(UNLOCKED, Ordering::Release);
        let addr = self.state.as_ptr() as usize;
        // SAFETY: We are unparking a thread waiting on the address of the state atomic.
        unsafe {
            unpark_one(addr, |_| DEFAULT_UNPARK_TOKEN);
        }
    }
}

pub(crate) struct SpinFreeMutexGuard<'a, T> {
    lock: &'a SpinFreeMutex<T>,
}

impl<T> Deref for SpinFreeMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: We hold the lock, so we have exclusive access to the data.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinFreeMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: We hold the lock, so we have exclusive access to the data.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinFreeMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    #[test]
    fn mutex_simple() {
        let m = SpinFreeMutex::new(0);
        {
            let mut g = m.lock();
            *g += 1;
        }
        assert_eq!(*m.lock(), 1);
    }

    #[test]
    // Miri on Windows does not support GetModuleHandleA used by parking_lot_core in the contention path.
    #[cfg_attr(miri, ignore)]
    fn mutex_contention() {
        let m = Arc::new(SpinFreeMutex::new(0));
        let mut handles = vec![];

        for _ in 0..10 {
            let m = Arc::clone(&m);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let mut g = m.lock();
                    *g += 1;
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(*m.lock(), 1000);
    }
}
