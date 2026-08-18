use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::{fmt, mem};

/// Creates a [`Waker`] that runs `action` the first time it is cloned, returning it alongside a
/// flag that records whether the action has run.
///
/// An async primitive registers an awaiting task by cloning the waker it was handed, so the clone
/// is user code that runs while the primitive is midway through its own registration logic. This
/// waker is how a callback-safety test re-enters a primitive at exactly that moment, typically to
/// complete or cancel the very operation that is registering.
///
/// Only the first clone runs the action, so the action may consume values that exist once, such as
/// the sender endpoint of a one-shot event. Every other waker operation merely maintains the
/// reference count of the shared payload.
///
/// The flag lets a test prove that the reentrancy it targets actually occurred, instead of passing
/// vacuously because the primitive never cloned the waker.
///
/// # Safety
///
/// The action is not `Send`, so the returned waker and every clone of it must be used and dropped
/// on the calling thread. [`Waker`] is `Send + Sync` regardless of its payload, so this cannot be
/// expressed in the type system.
#[must_use]
pub unsafe fn clone_action_waker(action: impl FnOnce() + 'static) -> (Waker, Rc<Cell<bool>>) {
    let has_run = Rc::new(Cell::new(false));

    // The payload is deliberately single-threaded, as the safety contract above requires, while
    // the atomic reference counting keeps the waker's clone and drop operations thread-safe on
    // their own - the same split `drop_waker()` relies on.
    #[expect(
        clippy::arc_with_non_send_sync,
        reason = "the reference count must be atomic even though the payload stays on one thread"
    )]
    let data = Arc::new(CloneAction {
        action: RefCell::new(Some(Box::new(action))),
        has_run: Rc::clone(&has_run),
    });

    // SAFETY: The vtable upholds the `RawWaker` contract for an `Arc<CloneAction>` payload - it is
    // only ever paired with a pointer from `Arc::into_raw` and every reference count operation is
    // an atomic one. The caller upholds the remaining requirement, that the payload only travels
    // where it may travel.
    let waker = unsafe { Waker::from_raw(raw_waker(data)) };

    (waker, has_run)
}

/// Carries the one-shot action of a [`clone_action_waker`] waker, shared by every clone of it.
struct CloneAction {
    action: RefCell<Option<Box<dyn FnOnce()>>>,
    has_run: Rc<Cell<bool>>,
}

impl CloneAction {
    /// Runs the action, if it has not already run.
    fn run(&self) {
        // The action is taken out before it runs, so that the borrow is released before user code
        // that may clone the waker again observes the payload.
        // Ref: docs/callback-safety.md, "No callbacks under borrows of shared state".
        let Some(action) = self.action.borrow_mut().take() else {
            return;
        };

        action();

        self.has_run.set(true);
    }
}

fn raw_waker(data: Arc<CloneAction>) -> RawWaker {
    RawWaker::new(Arc::into_raw(data).cast(), &VTABLE)
}

/// # Safety
///
/// `data` must come from `Arc::<CloneAction>::into_raw()` and its reference must still be owned by
/// the waker being cloned.
unsafe fn clone_raw(data: *const ()) -> RawWaker {
    // SAFETY: Forwarding the guarantees of the caller.
    let arc = unsafe { Arc::from_raw(data.cast::<CloneAction>()) };

    let clone = Arc::clone(&arc);

    // The reference we reconstructed still belongs to the waker we were cloned from, so we must
    // not release it here.
    mem::forget(arc);

    // The clone already owns its reference, so the action is free to drive the primitive that is
    // cloning us all the way to completion, including paths that release this very waker.
    clone.run();

    raw_waker(clone)
}

/// # Safety
///
/// `data` must come from `Arc::<CloneAction>::into_raw()` and its reference must be consumed by
/// this call.
unsafe fn wake_raw(data: *const ()) {
    // SAFETY: Forwarding the guarantees of the caller.
    unsafe { drop_raw(data) }
}

/// Waking by reference does not consume a reference, so there is nothing to release.
fn wake_by_ref_raw(_data: *const ()) {}

/// # Safety
///
/// `data` must come from `Arc::<CloneAction>::into_raw()` and its reference must be consumed by
/// this call.
unsafe fn drop_raw(data: *const ()) {
    // SAFETY: Forwarding the guarantees of the caller.
    let arc = unsafe { Arc::from_raw(data.cast::<CloneAction>()) };

    drop(arc);
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

impl fmt::Debug for CloneAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("has_run", &self.has_run.get())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_clone_runs_the_action() {
        let runs = Rc::new(Cell::new(0_usize));
        let runs_in_action = Rc::clone(&runs);

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, has_run) = unsafe {
            clone_action_waker(move || runs_in_action.set(runs_in_action.get().saturating_add(1)))
        };

        assert!(!has_run.get());

        let clone = waker.clone();
        assert!(has_run.get());
        assert_eq!(runs.get(), 1);

        // Later clones share the payload but find the action already consumed.
        let another_clone = clone.clone();
        assert_eq!(runs.get(), 1);

        drop(another_clone);
        clone.wake();
        waker.wake_by_ref();
        drop(waker);

        assert_eq!(runs.get(), 1);
    }

    #[test]
    fn unused_waker_never_runs_the_action() {
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, has_run) = unsafe { clone_action_waker(|| unreachable!("never cloned")) };

        waker.wake();

        assert!(!has_run.get());
    }
}
