use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::mem;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable, Waker};

/// Creates an owned [`Waker`] that runs one action the first time it is woken.
///
/// The raw waker owns its payload through an [`Arc`], so Miri can validate reentrant accesses made
/// by the action without relying on a borrowed raw pointer. The returned flag records whether a
/// wake callback ran and lets tests reject vacuous passes.
///
/// # Safety
///
/// The action is not `Send`, so the returned waker and every clone of it must be used and dropped
/// on the calling thread. [`Waker`] is `Send + Sync` regardless of its payload, so this cannot be
/// expressed in the type system.
#[must_use]
pub unsafe fn wake_action_waker(action: impl FnOnce() + 'static) -> (Waker, Rc<Cell<bool>>) {
    let was_woken = Rc::new(Cell::new(false));

    // The raw-waker contract requires atomic reference counting even though this payload stays on
    // one thread under the function's safety contract.
    #[expect(
        clippy::arc_with_non_send_sync,
        reason = "RawWaker clones require atomic reference counting for their owned payload"
    )]
    let data = Arc::new(WakeAction {
        action: RefCell::new(Some(Box::new(action))),
        was_woken: Rc::clone(&was_woken),
    });

    // SAFETY: The vtable only receives pointers produced by `Arc::into_raw`, and each operation
    // reconstructs, clones or consumes exactly the reference the RawWaker contract assigns it.
    // The caller keeps the non-Send payload on this thread.
    let waker = unsafe { Waker::from_raw(raw_waker(data)) };

    (waker, was_woken)
}

fn raw_waker(data: Arc<WakeAction>) -> RawWaker {
    RawWaker::new(Arc::into_raw(data).cast(), &WakeActionVTable::VTABLE)
}

/// Owns the one-shot wake callback shared by every raw-waker reference.
struct WakeAction {
    action: RefCell<Option<Box<dyn FnOnce()>>>,
    was_woken: Rc<Cell<bool>>,
}

impl WakeAction {
    fn run(&self) {
        self.was_woken.set(true);

        // Release the borrow before user code runs so the action may re-enter code that wakes or
        // clones this waker.
        let Some(action) = self.action.borrow_mut().take() else {
            return;
        };

        action();
    }
}

/// Carries the raw-waker operations for the [`WakeAction`] payload.
struct WakeActionVTable(PhantomData<WakeAction>);

impl WakeActionVTable {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_raw,
        Self::wake_raw,
        Self::wake_by_ref_raw,
        Self::drop_raw,
    );

    /// # Safety
    ///
    /// `data` must come from `Arc::<WakeAction>::into_raw()` and its reference must still belong
    /// to the waker being cloned.
    unsafe fn clone_raw(data: *const ()) -> RawWaker {
        // SAFETY: Forwarding the caller's pointer provenance and live-reference guarantee.
        let data = unsafe { Arc::from_raw(data.cast::<WakeAction>()) };
        let clone = Arc::clone(&data);
        mem::forget(data);

        raw_waker(clone)
    }

    /// # Safety
    ///
    /// `data` must come from `Arc::<WakeAction>::into_raw()` and its reference must be consumed by
    /// this call.
    unsafe fn wake_raw(data: *const ()) {
        // SAFETY: Forwarding the caller's pointer provenance and ownership guarantee.
        let data = unsafe { Arc::from_raw(data.cast::<WakeAction>()) };
        data.run();
    }

    /// # Safety
    ///
    /// `data` must come from `Arc::<WakeAction>::into_raw()` and its reference must remain owned by
    /// the waker after this call.
    unsafe fn wake_by_ref_raw(data: *const ()) {
        // SAFETY: Forwarding the caller's pointer provenance and live-reference guarantee.
        let data = unsafe { Arc::from_raw(data.cast::<WakeAction>()) };
        data.run();

        _ = Arc::into_raw(data);
    }

    /// # Safety
    ///
    /// `data` must come from `Arc::<WakeAction>::into_raw()` and its reference must be consumed by
    /// this call.
    unsafe fn drop_raw(data: *const ()) {
        // SAFETY: Forwarding the caller's pointer provenance and ownership guarantee.
        let data = unsafe { Arc::from_raw(data.cast::<WakeAction>()) };
        drop(data);
    }
}
