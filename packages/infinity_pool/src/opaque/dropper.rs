use std::mem;
use std::ptr::{self, NonNull};

/// Remembers how to drop an object while forgetting its type.
///
/// Drops its target when it is itself dropped.
#[derive(Debug)]
pub(crate) struct Dropper {
    ptr: NonNull<()>,

    // `None` when `T` does not need dropping (e.g. primitives, `Copy` types). Storing `None`
    // avoids an indirect call through a fn pointer that resolves to a no-op `drop_in_place`,
    // saving instructions on every handle remove for trivially-droppable payloads.
    //
    // `fn` pointers have a null niche, so wrapping in `Option` does not grow `Dropper`.
    drop_fn: Option<fn(NonNull<()>)>,
}

impl Dropper {
    /// Creates a new `Dropper` that will drop the pinned `T` referenced when the dropper
    /// itself is dropped.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// 1. The target pointer is valid for `T` writes for the lifetime of the `Dropper` instance.
    /// 2. The target is not dropped through its normal lifetime (e.g., by calling `drop()`
    ///    or letting it go out of scope) while the `Dropper` exists.
    /// 3. Only one `Dropper` instance exists for any given target at a time.
    pub(crate) unsafe fn new<T>(target: NonNull<T>) -> Self {
        let drop_fn = if mem::needs_drop::<T>() {
            let drop_fn = drop_fn::<T>;

            // Erase the type of the pointer in the function arguments.
            // SAFETY: We are just changing the target of the pointer arg, everything is ABI-equal.
            let drop_fn = unsafe { mem::transmute::<fn(NonNull<T>), fn(NonNull<()>)>(drop_fn) };

            Some(drop_fn)
        } else {
            None
        };

        Self {
            ptr: target.cast(),
            drop_fn,
        }
    }
}

impl Drop for Dropper {
    // Without `#[inline]`, `Slab::remove` lowers the dropper invocation to a PLT/dynamic-linker
    // indirect call (verified in disassembly). The body is small — a function-pointer null check
    // plus an indirect call to the stored drop function — and inlining it removes that overhead
    // for every handle drop. See issue #192.
    #[inline]
    fn drop(&mut self) {
        if let Some(drop_fn) = self.drop_fn {
            drop_fn(self.ptr);
        }
    }
}

fn drop_fn<T>(ptr: NonNull<T>) {
    // SAFETY: Dropper::new() ensures safety requirements are met.
    unsafe {
        ptr::drop_in_place(ptr.as_ptr());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::mem;
    use std::rc::Rc;

    use static_assertions::const_assert_eq;

    use super::*;

    // The whole point of using `Option<fn(_)>` instead of a separate `bool` is to exploit the
    // null niche of function pointers. If this assertion breaks, the optimization has
    // regressed and `Dropper` grew, which would in turn grow `SlotMeta`.
    const_assert_eq!(
        size_of::<Option<fn(NonNull<()>)>>(),
        size_of::<fn(NonNull<()>)>()
    );

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

        // Create dropper for the pinned object.
        // SAFETY: We ensure the target outlives the dropper and prevent double-drop with mem::forget.
        let dropper = unsafe { Dropper::new(NonNull::from(&mut stack_value)) };

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
        mem::forget(stack_value);
    }

    #[test]
    fn dropper_handles_zero_sized_types() {
        let (tracker, dropped_flag) = DropTracker::new();
        let mut stack_value = ((), tracker);

        // SAFETY: We ensure the target outlives the dropper and prevent double-drop with mem::forget.
        let dropper = unsafe { Dropper::new(NonNull::from(&mut stack_value)) };

        assert!(!dropped_flag.get());
        drop(dropper);
        assert!(dropped_flag.get());

        // Prevent double-drop by forgetting the original.
        mem::forget(stack_value);
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

        {
            // SAFETY: We ensure the target outlives the dropper and prevent double-drop with mem::forget.
            let dropper = unsafe { Dropper::new(NonNull::from(&mut stack_complex)) };
            assert!(!dropped_flag.get());
            drop(dropper);
        }

        assert!(dropped_flag.get());

        // Prevent double-drop by forgetting the original.
        mem::forget(stack_complex);
    }

    #[test]
    fn dropper_skips_drop_for_trivial_types() {
        // For a type that does not need dropping, we expect the dropper to store `None`
        // and short-circuit on drop — no `drop_in_place` call should be made through the
        // function pointer.
        let mut value: u64 = 0x1234_5678_9abc_def0;

        // SAFETY: `u64` is `Copy` and does not need dropping; the target outlives the dropper.
        let dropper = unsafe { Dropper::new(NonNull::from(&mut value)) };

        assert!(
            dropper.drop_fn.is_none(),
            "Dropper for !needs_drop type should not carry a drop function"
        );

        drop(dropper);

        // Value should still be intact since no drop logic ran against it.
        assert_eq!(value, 0x1234_5678_9abc_def0);
    }

    #[test]
    fn dropper_records_drop_fn_for_droppable_types() {
        let (tracker, _dropped_flag) = DropTracker::new();
        let mut stack_value = tracker;

        // SAFETY: target outlives dropper; we mem::forget the original below to prevent double-drop.
        let dropper = unsafe { Dropper::new(NonNull::from(&mut stack_value)) };

        assert!(
            dropper.drop_fn.is_some(),
            "Dropper for needs_drop type should carry a drop function"
        );

        drop(dropper);

        // Prevent double-drop by forgetting the original.
        mem::forget(stack_value);
    }
}
