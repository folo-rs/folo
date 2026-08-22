use super::*;

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_no_awaiter() {
    let (place, _sender, _receiver) = placed::<i32>();

    let backtrace = placed_event(&place).awaiter_backtrace();

    assert!(backtrace.is_none());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_with_awaiter() {
    let (place, _sender, receiver) = placed::<i32>();

    let mut cx = task::Context::from_waker(Waker::noop());
    let mut receiver = Box::pin(receiver);
    _ = receiver.as_mut().poll(&mut cx);

    let backtrace = placed_event(&place).awaiter_backtrace();

    assert!(backtrace.is_some());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_after_sender_drop() {
    let (place, sender, receiver) = placed::<i32>();

    let mut cx = task::Context::from_waker(Waker::noop());
    let mut receiver = Box::pin(receiver);
    _ = receiver.as_mut().poll(&mut cx);

    drop(sender);

    let backtrace = placed_event(&place).awaiter_backtrace();

    assert!(backtrace.is_some());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_after_receiver_drop() {
    let (place, _sender, receiver) = placed::<i32>();

    let mut cx = task::Context::from_waker(Waker::noop());
    let mut receiver = Box::pin(receiver);
    _ = receiver.as_mut().poll(&mut cx);

    drop(receiver);

    let backtrace = placed_event(&place).awaiter_backtrace();

    assert!(backtrace.is_some());
}

#[cfg(debug_assertions)]
#[test]
fn awaiter_backtrace_outlives_event() {
    let backtrace = {
        let (place, _sender, receiver) = placed::<i32>();

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        placed_event(&place)
            .awaiter_backtrace()
            .expect("the event has been awaited")
    };

    // The event storage is gone but the snapshot remains readable.
    _ = backtrace.status();
}

#[cfg(debug_assertions)]
#[test]
fn released_event_releases_backtrace() {
    let (place, sender, receiver) = placed::<i32>();

    {
        // Both endpoints go out of scope at the end of this block, which releases the event
        // while its storage stays ours.
        let _sender = sender;
        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);
    }

    // The event has been released but its storage is still ours to inspect. Releasing an
    // event releases its backtrace, because the storage may be reused without dropping it.
    let event = placed_event(&place);

    assert!(event.awaiter_backtrace().is_none());
}
