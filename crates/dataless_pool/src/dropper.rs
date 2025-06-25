#![allow(dead_code, reason = "soon")]

use std::pin::Pin;
use std::ptr::{self, NonNull};

/// Remembers how to drop an object while forgetting its type.
///
/// Drops its target when it is itself dropped.
#[derive(Debug)]
pub(crate) struct Dropper {
    ptr: NonNull<()>,
    drop_fn: fn(NonNull<()>),
}

impl Dropper {
    /// Creates a new `Dropper` that will drop the pinned `T` referenced when the dropper
    /// itself is dropped.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// 1. The target outlives the `Dropper` instance.
    /// 2. The target is not dropped through its normal lifetime (e.g., by calling `drop()`
    ///    or letting it go out of scope) while the `Dropper` exists.
    /// 3. Only one `Dropper` instance exists for any given target at a time.
    pub(crate) unsafe fn new<T>(target: Pin<&mut T>) -> Self {
        let drop_fn = drop_fn::<T>;

        // Erase the type of the pointer.
        // SAFETY: We are just changing the target of the pointer arg, everything is ABI-equal.
        let drop_fn = unsafe { std::mem::transmute::<fn(NonNull<T>), fn(NonNull<()>)>(drop_fn) };

        // SAFETY: We are not moving anything, just creating a pointer to the pinned object.
        let ptr = NonNull::from(unsafe { target.get_unchecked_mut() });

        Self {
            ptr: ptr.cast(),
            drop_fn,
        }
    }
}

impl Drop for Dropper {
    fn drop(&mut self) {
        (self.drop_fn)(self.ptr);
    }
}

fn drop_fn<T>(ptr: NonNull<T>) {
    // SAFETY: Dropper::new() ensures safety requirements are met.
    unsafe {
        ptr::drop_in_place(ptr.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    /// Test helper that tracks whether it has been dropped.
    struct DropTracker {
        dropped: Rc<Cell<bool>>,
    }

    impl DropTracker {
        fn new() -> (Self, Rc<Cell<bool>>) {
            let dropped = Rc::new(Cell::new(false));
            (
                Self {
                    dropped: Rc::clone(&dropped),
                },
                dropped,
            )
        }
    }

    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }

    #[test]
    fn dropper_drops_target_when_dropped() {
        let (tracker, dropped_flag) = DropTracker::new();
        let mut stack_value = tracker;
        let mut pinned = Pin::new(&mut stack_value);

        // Create dropper for the pinned object.
        // SAFETY: We ensure the target outlives the dropper and prevent double-drop with mem::forget.
        let dropper = unsafe { Dropper::new(pinned.as_mut()) };

        // Target should not be dropped yet.
        assert!(
            !dropped_flag.get(),
            "Target should not be dropped before dropper is dropped"
        );

        // Drop the dropper.
        drop(dropper);

        // Target should now be dropped.
        assert!(
            dropped_flag.get(),
            "Target should be dropped after dropper is dropped"
        );

        // Prevent double-drop by forgetting the original.
        std::mem::forget(stack_value);
    }

    #[test]
    fn dropper_handles_zero_sized_types() {
        let (tracker, dropped_flag) = DropTracker::new();
        let mut stack_value = ((), tracker);
        let mut pinned = Pin::new(&mut stack_value);

        // SAFETY: We ensure the target outlives the dropper and prevent double-drop with mem::forget.
        let dropper = unsafe { Dropper::new(pinned.as_mut()) };

        assert!(!dropped_flag.get());
        drop(dropper);
        assert!(dropped_flag.get());

        // Prevent double-drop by forgetting the original.
        std::mem::forget(stack_value);
    }

    #[test]
    fn dropper_works_with_complex_types() {
        struct ComplexType {
            _data: Vec<String>,
            _nested: Vec<i32>,
            tracker: Rc<Cell<bool>>,
        }

        impl Drop for ComplexType {
            fn drop(&mut self) {
                self.tracker.set(true);
            }
        }

        let dropped_flag = Rc::new(Cell::new(false));
        let complex = ComplexType {
            _data: vec!["hello".to_string(), "world".to_string()],
            _nested: vec![1, 2, 3, 4, 5],
            tracker: Rc::clone(&dropped_flag),
        };

        let mut stack_complex = complex;
        let mut pinned = Pin::new(&mut stack_complex);

        {
            // SAFETY: We ensure the target outlives the dropper and prevent double-drop with mem::forget.
            let dropper = unsafe { Dropper::new(pinned.as_mut()) };
            assert!(!dropped_flag.get());
            drop(dropper);
        }

        assert!(dropped_flag.get());

        // Prevent double-drop by forgetting the original.
        std::mem::forget(stack_complex);
    }
}
