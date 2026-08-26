use super::*;

/// Returns a shared reference to the event placed in `place`, so a test can inspect
/// diagnostic state that the endpoints do not expose.
///
/// # Safety
///
/// An event must have been placed in `place` by [`Event::placed`] and that storage must not
/// have been reused for another event since. No exclusive reference to the event storage may
/// exist for the lifetime of the returned reference.
#[cfg(debug_assertions)]
unsafe fn placed_event(place: &EmbeddedEvent<i32>) -> &Event<i32> {
    // SAFETY: The caller guarantees that an event was placed in this storage, so the pointer
    // is non-null and aligned for `Event<i32>`, and the storage lives as long as the borrow
    // of the container. Every access to a placed event - here and in the endpoints - goes
    // through shared references, and the caller guarantees that no exclusive reference to the
    // storage exists, so this shared reference cannot conflict with another borrow.
    let cell = unsafe { place.inner.get().as_ref() }.expect("UnsafeCell pointer is never null");

    // SAFETY: `Event::placed` initialized the event in this storage. Releasing an event only
    // clears its diagnostic state and never deinitializes those bytes, so the event remains
    // initialized for as long as the storage exists.
    unsafe { cell.assume_init_ref() }
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_no_awaiter() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let _endpoints = unsafe { Event::<i32>::placed(place.as_mut()) };

    // SAFETY: We placed an event in this storage just above and did not reuse the storage.
    // The endpoints hold only raw pointers and the placement borrow has ended, so there is no
    // exclusive reference to the storage.
    let backtrace = unsafe { placed_event(&place) }.awaiter_backtrace();

    assert!(backtrace.is_none());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_with_awaiter() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (_sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

    let mut cx = task::Context::from_waker(Waker::noop());
    let mut receiver = Box::pin(receiver);
    _ = receiver.as_mut().poll(&mut cx);

    // SAFETY: We placed an event in this storage above and did not reuse the storage. The
    // endpoints hold only raw pointers and the placement borrow has ended, so there is no
    // exclusive reference to the storage.
    let backtrace = unsafe { placed_event(&place) }.awaiter_backtrace();

    assert!(backtrace.is_some());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_after_sender_drop() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

    let mut cx = task::Context::from_waker(Waker::noop());
    let mut receiver = Box::pin(receiver);
    _ = receiver.as_mut().poll(&mut cx);

    drop(sender);

    // SAFETY: We placed an event in this storage above and did not reuse the storage. The
    // receiver is still an endpoint of it and holds only a raw pointer, so there is no
    // exclusive reference to the storage.
    let backtrace = unsafe { placed_event(&place) }.awaiter_backtrace();

    assert!(backtrace.is_some());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_after_receiver_drop() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (_sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

    let mut cx = task::Context::from_waker(Waker::noop());
    let mut receiver = Box::pin(receiver);
    _ = receiver.as_mut().poll(&mut cx);

    drop(receiver);

    // SAFETY: We placed an event in this storage above and did not reuse the storage. The
    // sender is still an endpoint of it and holds only a raw pointer, so there is no
    // exclusive reference to the storage.
    let backtrace = unsafe { placed_event(&place) }.awaiter_backtrace();

    assert!(backtrace.is_some());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_outlives_event() {
    let backtrace = {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
        // its storage stays allocated, writable and at a stable address until the end of
        // the test, which outlives both endpoints.
        let (_sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        // SAFETY: We placed an event in this storage above and did not reuse the storage.
        // The endpoints hold only raw pointers and the placement borrow has ended, so there
        // is no exclusive reference to the storage.
        unsafe { placed_event(&place) }
            .awaiter_backtrace()
            .expect("the event has been awaited")
    };

    // The event storage is gone but the snapshot remains readable.
    _ = backtrace.status();
}

#[cfg(debug_assertions)]
#[test]
fn released_event_releases_backtrace() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());

    {
        // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
        // its storage stays allocated, writable and at a stable address until the end of
        // the test, which outlives both endpoints.
        let (_sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);
    }

    // The event has been released but its storage is still ours to inspect. Releasing an
    // event releases its backtrace, because the storage may be reused without dropping it.
    //
    // SAFETY: We placed an event in this storage above and did not reuse the storage.
    // Releasing the event left it initialized, both endpoints are gone and nothing holds an
    // exclusive reference to the storage.
    let event = unsafe { placed_event(&place) };

    assert!(event.awaiter_backtrace().is_none());
}
