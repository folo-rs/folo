use super::*;

#[test]
fn disconnected_send_releases_slot_when_payload_drop_panics() {
    let pool = Box::pin(RawEventPool::<PanicsOnDrop>::new());
    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    drop(receiver);

    assert_panics(|| sender.send(PanicsOnDrop));
    assert!(pool.is_empty());
}

#[test]
fn receiver_drop_releases_slot_when_waker_drop_panics() {
    let pool = Box::pin(RawEventPool::<i32>::new());
    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker_panicking_on_clone_release(|| {}) };
    let mut cx = task::Context::from_waker(&waker);
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
    assert!(cloned.get());
    drop(waker);

    assert_panics(|| drop(receiver));
    drop(sender);

    assert!(pool.is_empty());
}

#[test]
fn concurrent_rentals_share_pool_core_mt() {
    // The smallest group that guarantees competing rentals.
    const WORKER_COUNT: usize = 2;

    with_watchdog(|| {
        let pool = Box::pin(RawEventPool::<i32>::new());
        let barrier = Barrier::new(WORKER_COUNT);

        thread::scope(|scope| {
            for _ in 0..WORKER_COUNT {
                let pool = pool.as_ref();
                let barrier = &barrier;

                scope.spawn(move || {
                    barrier.wait();

                    // SAFETY: The enclosing scope joins every worker before the pinned pool
                    // is dropped, so the pool outlives the rented endpoints.
                    drop(unsafe { pool.rent() });
                });
            }
        });

        assert!(pool.is_empty());
    });
}

#[test]
fn len() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    assert_eq!(pool.len(), 0);

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender1, receiver1) = unsafe { pool.as_ref().rent() };
    assert_eq!(pool.len(), 1);

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender2, receiver2) = unsafe { pool.as_ref().rent() };
    assert_eq!(pool.len(), 2);

    drop(sender1);
    drop(receiver1);
    assert_eq!(pool.len(), 1);

    drop(sender2);
    drop(receiver2);
    assert_eq!(pool.len(), 0);
}

#[test]
fn send_receive() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    assert!(pool.is_empty());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };

    assert!(!pool.is_empty());

    {
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    assert!(pool.is_empty());
}

#[test]
fn send_receive_reused() {
    const ITERATIONS: usize = 32;

    let pool = Box::pin(RawEventPool::<i32>::new());

    assert!(pool.is_empty());

    for _ in 0..ITERATIONS {
        // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    assert!(pool.is_empty());
}

#[test]
fn send_receive_reused_batches() {
    const ITERATIONS: usize = 4;
    const BATCH_SIZE: usize = 8;

    let pool = Box::pin(RawEventPool::<i32>::new());

    for _ in 0..ITERATIONS {
        // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
        let endpoints = iter::repeat_with(|| unsafe { pool.as_ref().rent() })
            .take(BATCH_SIZE)
            .collect::<Vec<_>>();

        for (sender, receiver) in endpoints {
            let mut receiver = Box::pin(receiver);

            sender.send(42);

            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        }
    }
}

#[test]
fn drop_send() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, _) = unsafe { pool.as_ref().rent() };

    sender.send(42);
}

#[test]
fn drop_receive() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (_, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn receive_drop_receive() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn receive_drop_send() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);

    sender.send(42);
}

#[test]
fn receive_drop_drop_receiver_first() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);
    drop(sender);
}

#[test]
fn receive_drop_drop_sender_first() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);
    drop(receiver);
}

#[test]
fn drop_drop_receiver_first() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };

    drop(receiver);
    drop(sender);
}

#[test]
fn drop_drop_sender_first() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };

    drop(sender);
    drop(receiver);
}

#[test]
fn is_ready() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    assert!(!receiver.is_ready());

    sender.send(42);

    assert!(receiver.is_ready());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn drop_is_ready() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    assert!(!receiver.is_ready());

    drop(sender);

    assert!(receiver.is_ready());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn into_value() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };

    let Err(crate::IntoValueError::Pending(receiver)) = receiver.into_value() else {
        panic!("Expected receiver to not be ready");
    };

    sender.send(42);

    assert!(matches!(receiver.into_value(), Ok(42)));
}

#[test]
#[should_panic]
fn panic_poll_after_completion() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));

    _ = receiver.as_mut().poll(&mut cx);
}

#[test]
#[should_panic]
fn panic_is_ready_after_completion() {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: The pinned pool remains alive until both returned endpoints are dropped.
    let (sender, receiver) = unsafe { pool.as_ref().rent() };
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));

    _ = receiver.is_ready();
}
