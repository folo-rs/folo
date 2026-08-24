#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
#[cfg(debug_assertions)]
use std::cell::Cell;
#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::pin::Pin;
#[cfg(test)]
use std::task::{self, Poll};

#[cfg(test)]
use testing::{assert_panics, clone_action_waker_panicking_on_clone_release};

/// Arbitrary test payload; event lifecycle logic treats its value as opaque.
#[cfg(test)]
const TEST_PAYLOAD: i32 = 42;

/// The closure that an `inspect_awaiters()` method calls once per awaited event.
#[cfg(debug_assertions)]
type AwaiterInspector<'a> = dyn FnMut(&Backtrace) + 'a;

/// Calls the `inspect_awaiters()` method of the object under test, passing it the provided
/// closure.
#[cfg(debug_assertions)]
type InspectAwaiters<'a> = dyn Fn(&mut AwaiterInspector<'_>) + 'a;

/// Asserts that the `inspect_awaiters()` method of an event pool or event lake tolerates a
/// closure that re-enters the same pool or lake.
///
/// The object under test must already contain at least one awaited event, so that the closure
/// is called at least once.
///
/// `inspect` must call the `inspect_awaiters()` method of the object under test with the closure
/// it receives. `rent_and_drop` must rent an event from the same object and immediately drop
/// both of its endpoints, returning the event to the object.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(debug_assertions)]
pub(crate) fn assert_inspect_awaiters_is_reentrant(
    inspect: &InspectAwaiters<'_>,
    rent_and_drop: &dyn Fn(),
) {
    // The nested inspection is limited to the first call of the outer closure, so that both
    // inspections observe the same number of awaited events.
    const MAX_DEPTH: usize = 1;

    let depth = Cell::new(0_usize);
    let outer_calls = Cell::new(0_usize);
    let nested_calls = Cell::new(0_usize);

    inspect(&mut |_backtrace| {
        outer_calls.set(outer_calls.get().saturating_add(1));

        // Renting an event and returning it exercises the mutating paths of the pool or lake.
        rent_and_drop();

        if depth.get() < MAX_DEPTH {
            depth.set(depth.get().saturating_add(1));

            inspect(&mut |_backtrace| {
                nested_calls.set(nested_calls.get().saturating_add(1));
            });
        }
    });

    assert!(outer_calls.get() > 0);

    // The nested inspection observes the same awaited events as the outer one because the event
    // that `rent_and_drop` rents is never awaited and therefore never inspected.
    assert_eq!(nested_calls.get(), outer_calls.get());
}

/// A payload whose destructor unwinds after a disconnected send rejects it.
#[cfg(test)]
pub(crate) struct PanickingPayload;

#[cfg(test)]
impl Drop for PanickingPayload {
    fn drop(&mut self) {
        panic!("payload destructor");
    }
}

/// Asserts that a disconnected send returns event storage before dropping its payload.
#[cfg(test)]
pub(crate) fn assert_disconnected_send_payload_panic_releases_event<S, R>(
    rent: impl FnOnce() -> (S, R),
    send: impl FnOnce(S, PanickingPayload),
    is_empty: impl FnOnce() -> bool,
) {
    let (sender, receiver) = rent();
    drop(receiver);

    assert_panics(|| send(sender, PanickingPayload));

    assert!(
        is_empty(),
        "the event must be returned before the payload destructor unwinds"
    );
}

/// Asserts that receiver cancellation completes its handoff before dropping a stored waker.
#[cfg(test)]
pub(crate) fn assert_receiver_waker_panic_handoff_releases_event<S, R>(
    rent: impl FnOnce() -> (S, R),
    send: impl FnOnce(S, i32),
    is_empty: impl FnOnce() -> bool,
) where
    R: Future + Unpin,
{
    let (sender, mut receiver) = rent();

    // SAFETY: The payload is not `Send`, and this helper keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker_panicking_on_clone_release(|| {}) };

    let mut cx = task::Context::from_waker(&waker);
    assert!(matches!(
        Pin::new(&mut receiver).poll(&mut cx),
        Poll::Pending
    ));
    assert!(cloned.get(), "the receiver must register a waker clone");
    drop(waker);

    assert_panics(|| drop(receiver));

    send(sender, TEST_PAYLOAD);
    assert!(
        is_empty(),
        "the sender must observe receiver disconnection and return the event"
    );
}

/// Asserts that receiver teardown returns event storage before dropping an unread payload.
#[cfg(test)]
pub(crate) fn assert_unread_payload_panic_releases_event<S, R>(
    rent: impl FnOnce() -> (S, R),
    send: impl FnOnce(S, PanickingPayload),
    is_empty: impl FnOnce() -> bool,
) {
    let (sender, receiver) = rent();
    send(sender, PanickingPayload);

    assert_panics(|| drop(receiver));

    assert!(
        is_empty(),
        "the event must be returned before the unread payload destructor unwinds"
    );
}
