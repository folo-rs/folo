use super::*;

#[cfg(debug_assertions)]
#[test]
fn inspect_awaiters_inspects_only_awaited() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (_sender1, receiver1) = unsafe { pool.as_ref().rent() };
    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender2, receiver2) = unsafe { pool.as_ref().rent() };
    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (_sender3, _receiver3) = unsafe { pool.as_ref().rent() };

    let mut receiver1 = Box::pin(receiver1);
    let mut receiver2 = Box::pin(receiver2);

    let mut cx = task::Context::from_waker(Waker::noop());
    _ = receiver1.as_mut().poll(&mut cx);
    _ = receiver2.as_mut().poll(&mut cx);

    let mut inspected_count = 0;

    pool.inspect_awaiters(|_bt| {
        inspected_count += 1;
    });

    assert_eq!(inspected_count, 2);

    drop(sender2);
    drop(receiver2);

    let mut inspected_count = 0;

    pool.inspect_awaiters(|_bt| {
        inspected_count += 1;
    });

    assert_eq!(inspected_count, 1);
}

#[test]
fn default_creates_functional_pool() {
    let pool = Box::pin(RawEventPool::<i32>::default());

    assert!(pool.is_empty());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[cfg(debug_assertions)]
#[test]
fn inspect_awaiters_propagates_panic_from_closure() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pool outlives both endpoints.
    let (_sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());
    _ = receiver.as_mut().poll(&mut cx);

    assert_panics_with(
        || {
            pool.as_ref().inspect_awaiters(|_bt| {
                panic!("intentional panic to verify pass-through");
            });
        },
        |message| assert!(message.contains("pass-through")),
    );

    assert_eq!(pool.len(), 1);

    let mut inspected_count = 0;

    // Relocking the diagnostic registry proves that the panic released its prior lock.
    pool.inspect_awaiters(|_bt| {
        inspected_count += 1;
    });

    assert_eq!(inspected_count, 1);
}

#[cfg(debug_assertions)]
#[test]
fn inspect_awaiters_closure_may_reenter_pool() {
    // The callback re-enters the pool, so a regression that calls it under the pool lock
    // deadlocks on the non-reentrant mutex. The watchdog turns that into a bounded failure.
    with_watchdog(|| {
        let pool = Box::pin(RawEventPool::<i32>::new());

        // SAFETY: The pool outlives both endpoints.
        let (_sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);

        assert_inspect_awaiters_is_reentrant(&|f| pool.inspect_awaiters(f), &|| {
            // SAFETY: The pool outlives both endpoints.
            let (sender, receiver) = unsafe { pool.as_ref().rent() };
            drop(sender);
            drop(receiver);
        });
    });
}

#[cfg(debug_assertions)]
#[test]
fn inspect_awaiters_tolerates_endpoint_drop_from_closure() {
    const EVENT_COUNT: usize = 3;

    // Dropping endpoints from the callback returns events to the pool, which takes the pool
    // lock. The watchdog bounds the deadlock that a callback-under-lock regression causes.
    with_watchdog(|| {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let mut cx = task::Context::from_waker(Waker::noop());

        let mut endpoints = Vec::with_capacity(EVENT_COUNT);

        for _ in 0..EVENT_COUNT {
            // SAFETY: The pool outlives both endpoints.
            let (sender, receiver) = unsafe { pool.as_ref().rent() };
            let mut receiver = Box::pin(receiver);
            _ = receiver.as_mut().poll(&mut cx);
            endpoints.push((sender, receiver));
        }

        // The closure releases the events it is inspecting. The backtraces it receives are
        // snapshots, so they remain valid and each event is still visited exactly once.
        let endpoints = RefCell::new(endpoints);
        let mut inspected_count = 0;

        pool.inspect_awaiters(|_bt| {
            inspected_count += 1;
            drop(endpoints.borrow_mut().pop());
        });

        assert_eq!(inspected_count, EVENT_COUNT);
        assert!(pool.is_empty());
    });
}

#[cfg(debug_assertions)]
#[test]
fn released_event_releases_backtrace() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pool outlives both endpoints.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());
    _ = receiver.as_mut().poll(&mut cx);

    // The receiver leaves the event behind for the sender to release.
    drop(receiver);

    let mut backtraces = pool.awaiter_backtraces();
    assert_eq!(backtraces.len(), 1);

    let backtrace = backtraces.pop().expect("the event has been awaited");
    assert_eq!(Arc::strong_count(&backtrace), 2);

    drop(sender);

    // Releasing the event releases its backtrace, leaving the snapshot as the only owner.
    assert_eq!(Arc::strong_count(&backtrace), 1);
}
