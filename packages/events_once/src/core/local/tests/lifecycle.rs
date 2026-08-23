use super::*;

#[test]
fn boxed_send_receive() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_send_receive_unit() {
    let (sender, receiver) = LocalEvent::<()>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(()))));
}

#[test]
fn boxed_send_receive_u128() {
    let (sender, receiver) = LocalEvent::<u128>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_send_receive_array() {
    let (sender, receiver) = LocalEvent::<[u128; 4]>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send([42, 43, 44, 45]);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok([42, 43, 44, 45]))));
}

#[test]
fn boxed_receive_send_receive() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    sender.send(42);

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_drop_send() {
    let (sender, _) = LocalEvent::<i32>::boxed();

    sender.send(42);
}

#[test]
fn boxed_drop_receive() {
    let (_, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn boxed_receive_drop_receive() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn boxed_receive_drop_send() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);

    sender.send(42);
}

#[test]
fn boxed_receive_drop_drop_receiver_first() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);
    drop(sender);
}

#[test]
fn boxed_receive_drop_drop_sender_first() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);
    drop(receiver);
}

#[test]
fn boxed_drop_drop_receiver_first() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();

    drop(receiver);
    drop(sender);
}

#[test]
fn boxed_drop_drop_sender_first() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();

    drop(sender);
    drop(receiver);
}

#[test]
fn boxed_is_ready() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    assert!(!receiver.is_ready());

    sender.send(42);

    assert!(receiver.is_ready());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_drop_is_ready() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    assert!(!receiver.is_ready());

    drop(sender);

    assert!(receiver.is_ready());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn boxed_into_value() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();

    let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
        panic!("expected no value yet");
    };

    sender.send(42);

    assert!(matches!(receiver.into_value(), Ok(42)));
}

#[test]
fn boxed_drop_into_value() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();

    drop(sender);

    assert!(matches!(
        receiver.into_value(),
        Err(IntoValueError::Disconnected)
    ));
}

#[test]
#[should_panic]
fn boxed_panic_poll_after_completion() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));

    // Should panic - invalid to access receiver after it completes.
    _ = receiver.as_mut().poll(&mut cx);
}

#[test]
#[should_panic]
fn boxed_panic_is_ready_after_completion() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));

    // Should panic - invalid to access receiver after it completes.
    _ = receiver.is_ready();
}

#[test]
fn placed_send_receive() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn placed_receive_send_receive() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    sender.send(42);

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn placed_drop_send() {
    let (_place, sender, _) = placed::<i32>();

    sender.send(42);
}

#[test]
fn placed_drop_receive() {
    let (_place, _, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn placed_receive_drop_receive() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn placed_receive_drop_send() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);

    sender.send(42);
}

#[test]
fn placed_receive_drop_drop_receiver_first() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);
    drop(sender);
}

#[test]
fn placed_receive_drop_drop_sender_first() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);
    drop(receiver);
}

#[test]
fn placed_drop_drop_receiver_first() {
    let (_place, sender, receiver) = placed::<i32>();

    drop(receiver);
    drop(sender);
}

#[test]
fn placed_drop_drop_sender_first() {
    let (_place, sender, receiver) = placed::<i32>();

    drop(sender);
    drop(receiver);
}

#[test]
fn placed_is_ready() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    assert!(!receiver.is_ready());

    sender.send(42);

    assert!(receiver.is_ready());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn placed_drop_is_ready() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    assert!(!receiver.is_ready());

    drop(sender);

    assert!(receiver.is_ready());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn placed_into_value() {
    let (_place, sender, receiver) = placed::<i32>();

    let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
        panic!("expected no value yet");
    };

    sender.send(42);

    assert!(matches!(receiver.into_value(), Ok(42)));
}

#[test]
fn placed_drop_into_value() {
    let (_place, sender, receiver) = placed::<i32>();

    drop(sender);

    assert!(matches!(
        receiver.into_value(),
        Err(IntoValueError::Disconnected)
    ));
}

#[test]
#[should_panic]
fn placed_panic_poll_after_completion() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));

    // Should panic - invalid to access receiver after it completes.
    _ = receiver.as_mut().poll(&mut cx);
}

#[test]
#[should_panic]
fn placed_panic_is_ready_after_completion() {
    let (_place, sender, receiver) = placed::<i32>();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));

    // Should panic - invalid to access receiver after it completes.
    _ = receiver.is_ready();
}

#[test]
fn boxed_repoll_releases_previous_waker() {
    // A re-poll in the EVENT_AWAITING state must release the previously
    // registered waker exactly once when it replaces it. We observe this via
    // an `Arc`-backed waker whose strong count reflects the number of live
    // clones: a leaked registration would make the count grow with each re-poll.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    struct CountingWake {
        woken: AtomicUsize,
    }

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.woken.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.woken.fetch_add(1, Ordering::Relaxed);
        }
    }

    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let counter = Arc::new(CountingWake {
        woken: AtomicUsize::new(0),
    });
    let waker = Waker::from(Arc::clone(&counter));

    // Baseline: `counter` plus our local `waker`.
    assert_eq!(Arc::strong_count(&counter), 2);

    let mut cx = task::Context::from_waker(&waker);

    // First poll transitions BOUND → AWAITING and stores one clone.
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
    assert_eq!(Arc::strong_count(&counter), 3);

    // Each re-poll in the AWAITING state must drop the previous clone before
    // storing the new one, so the count stays put.
    for _ in 0..5 {
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
        assert_eq!(
            Arc::strong_count(&counter),
            3,
            "re-poll must release the previously registered waker",
        );
    }

    // Completion consumes and drops the stored waker, releasing its clone and
    // waking exactly the one registration that survived the replacements.
    sender.send(42);
    assert_eq!(Arc::strong_count(&counter), 2);
    assert_eq!(counter.woken.load(Ordering::Relaxed), 1);

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));
    assert_eq!(Arc::strong_count(&counter), 2);
}
