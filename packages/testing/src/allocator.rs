/// Miri-compatible backend for workspace executable allocators.
#[cfg(miri)]
pub use std::alloc::System as DefaultAllocator;

/// Allocator for workspace test, example and benchmark executables.
///
/// Native executables use mimalloc; Miri uses the system allocator because it
/// cannot execute mimalloc's native implementation. Allocation-tracking targets
/// wrap this allocator rather than replacing their instrumentation.
#[cfg(not(miri))]
pub use mimalloc::MiMalloc as DefaultAllocator;

/// Installs the workspace allocator in the calling executable.
///
/// Invoke once in each executable root, and behind `#[cfg(test)]` in a library
/// root. Merely depending on this crate does not select a global allocator.
/// Targets with allocation instrumentation install their own wrapper around
/// [`DefaultAllocator`] instead. Under Miri, this leaves the interpreter's default
/// allocator in place.
#[macro_export]
macro_rules! set_allocator {
    () => {
        #[cfg(not(miri))]
        const _: () = {
            #[global_allocator]
            static ALLOCATOR: $crate::DefaultAllocator = $crate::DefaultAllocator;
        };
    };
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::hint::black_box;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::thread;

    use super::*;

    static_assertions::assert_impl_all!(DefaultAllocator: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn selects_native_or_interpreted_backend() {
        #[cfg(not(miri))]
        assert_eq!(
            TypeId::of::<DefaultAllocator>(),
            TypeId::of::<mimalloc::MiMalloc>()
        );
        #[cfg(miri)]
        assert_eq!(
            TypeId::of::<DefaultAllocator>(),
            TypeId::of::<std::alloc::System>()
        );
    }

    #[test]
    fn global_allocations_can_grow_and_move_between_threads() {
        // Extend beyond the initial capacity, then free on a different thread.
        let values = black_box(vec![1_u64, 2, 3]);
        let values = thread::spawn(move || {
            let mut values = values;
            values.extend(4..=16);
            values
        })
        .join()
        .unwrap();
        assert!(values.into_iter().eq(1..=16));
    }
}
