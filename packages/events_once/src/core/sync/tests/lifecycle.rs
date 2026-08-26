use super::*;

#[test]
fn boxed_send_receive() {
    let (sender, receiver) = Event::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_send_receive_unit() {
    let (sender, receiver) = Event::<()>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(());

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(()))));
}

#[test]
fn boxed_send_receive_u128() {
    let (sender, receiver) = Event::<u128>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_send_receive_array() {
    let (sender, receiver) = Event::<[u128; 4]>::boxed();
    let mut receiver = Box::pin(receiver);

    sender.send([42, 43, 44, 45]);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok([42, 43, 44, 45]))));
}

#[test]
fn boxed_receive_send_receive() {
    let (sender, receiver) = Event::<i32>::boxed();
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
    let (sender, _) = Event::<i32>::boxed();

    sender.send(42);
}

#[test]
fn boxed_drop_receive() {
    let (_, receiver) = Event::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn boxed_receive_drop_receive() {
    let (sender, receiver) = Event::<i32>::boxed();
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
    let (sender, receiver) = Event::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);

    sender.send(42);
}

#[test]
fn boxed_receive_drop_drop_receiver_first() {
    let (sender, receiver) = Event::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);
    drop(sender);
}

#[test]
fn boxed_receive_drop_drop_sender_first() {
    let (sender, receiver) = Event::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);
    drop(receiver);
}

#[test]
fn boxed_drop_drop_receiver_first() {
    let (sender, receiver) = Event::<i32>::boxed();

    drop(receiver);
    drop(sender);
}

#[test]
fn boxed_drop_drop_sender_first() {
    let (sender, receiver) = Event::<i32>::boxed();

    drop(sender);
    drop(receiver);
}

#[test]
fn boxed_is_ready() {
    let (sender, receiver) = Event::<i32>::boxed();
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
    let (sender, receiver) = Event::<i32>::boxed();
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
    let (sender, receiver) = Event::<i32>::boxed();

    let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
        panic!("expected no value yet");
    };

    sender.send(42);

    assert!(matches!(receiver.into_value(), Ok(42)));
}

#[test]
fn boxed_drop_into_value() {
    let (sender, receiver) = Event::<i32>::boxed();

    drop(sender);

    assert!(matches!(
        receiver.into_value(),
        Err(IntoValueError::Disconnected)
    ));
}

#[test]
#[should_panic]
fn boxed_panic_poll_after_completion() {
    let (sender, receiver) = Event::<i32>::boxed();
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
    let (sender, receiver) = Event::<i32>::boxed();
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
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn placed_receive_send_receive() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
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
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, _) = unsafe { Event::<i32>::placed(place.as_mut()) };

    sender.send(42);
}

#[test]
fn placed_drop_receive() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (_, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

#[test]
fn placed_receive_drop_receive() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
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
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);

    sender.send(42);
}

#[test]
fn placed_receive_drop_drop_receiver_first() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(receiver);
    drop(sender);
}

#[test]
fn placed_receive_drop_drop_sender_first() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Pending));

    drop(sender);
    drop(receiver);
}

#[test]
fn placed_drop_drop_receiver_first() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

    drop(receiver);
    drop(sender);
}

#[test]
fn placed_drop_drop_sender_first() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

    drop(sender);
    drop(receiver);
}

#[test]
fn placed_is_ready() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
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
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
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
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

    let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
        panic!("expected no value yet");
    };

    sender.send(42);

    assert!(matches!(receiver.into_value(), Ok(42)));
}

#[test]
fn placed_drop_into_value() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

    drop(sender);

    assert!(matches!(
        receiver.into_value(),
        Err(IntoValueError::Disconnected)
    ));
}

#[test]
#[should_panic]
fn placed_panic_poll_after_completion() {
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
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
    let mut place = Box::pin(EmbeddedEvent::<i32>::new());
    // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
    // its storage stays allocated, writable and at a stable address until the end of
    // the test, which outlives both endpoints.
    let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
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
fn boxed_send_receive_mt() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();

        thread::spawn(move || {
            sender.send(42);
        })
        .join()
        .unwrap();

        thread::spawn(move || {
            let mut receiver = Box::pin(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        })
        .join()
        .unwrap();
    });
}

#[test]
fn boxed_receive_send_receive_mt() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();

        let first_poll_completed = Arc::new(Barrier::new(2));
        let first_poll_completed_clone = Arc::clone(&first_poll_completed);

        let send_thread = thread::spawn(move || {
            first_poll_completed.wait();

            sender.send(42);
        });

        let receive_thread = thread::spawn(move || {
            let mut receiver = Box::pin(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            first_poll_completed_clone.wait();

            // We do not know how many polls this will take, so we switch into real async.
            block_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}

#[test]
fn boxed_send_receive_unbiased_mt() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();

        let receive_thread = thread::spawn(move || {
            block_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}

#[test]
fn boxed_drop_receive_unbiased_mt() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();

        let receive_thread = thread::spawn(move || {
            block_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Err(Disconnected)));
            });
        });

        let send_thread = thread::spawn(move || {
            drop(sender);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}

#[test]
fn boxed_drop_send_unbiased_mt() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();

        let receive_thread = thread::spawn(move || {
            drop(receiver);
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}

#[test]
fn placed_send_receive_mt() {
    with_watchdog(|| {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
        // its storage stays allocated, writable and at a stable address until the end of
        // the test, which outlives both endpoints.
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        thread::spawn(move || {
            sender.send(42);
        })
        .join()
        .unwrap();

        thread::spawn(move || {
            let mut receiver = Box::pin(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        })
        .join()
        .unwrap();
    });
}

// The repeated thread creation and storage reuse in this test is native-run stress: every
// iteration exercises the same lifecycle, so interpreting all of them under each Miri
// scheduler seed costs a great deal without reaching new behavior. Miri still covers a single
// placed lifecycle via `placed_send_receive_mt` and the other placed tests.
#[test]
#[cfg_attr(miri, ignore)] // Repeated thread creation and reuse loop is for native runs only.
fn placed_send_receive_reused_mt() {
    with_watchdog(|| {
        const ITERATIONS: usize = 123;

        let mut place = Box::pin(EmbeddedEvent::<i32>::new());

        for _ in 0..ITERATIONS {
            // SAFETY: `place` is box-pinned, so its storage stays allocated, writable and at
            // a stable address until the end of the test, which outlives every endpoint. The
            // endpoints of the previous iteration are both dropped before we get here, so no
            // other event is using this storage.
            let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

            thread::spawn(move || {
                sender.send(42);
            })
            .join()
            .unwrap();

            thread::spawn(move || {
                let mut receiver = Box::pin(receiver);
                let mut cx = task::Context::from_waker(Waker::noop());

                let poll_result = receiver.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Ready(Ok(42))));
            })
            .join()
            .unwrap();
        }
    });
}

#[test]
fn placed_receive_send_receive_mt() {
    with_watchdog(|| {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
        // its storage stays allocated, writable and at a stable address until the end of
        // the test, which outlives both endpoints.
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let first_poll_completed = Arc::new(Barrier::new(2));
        let first_poll_completed_clone = Arc::clone(&first_poll_completed);

        let send_thread = thread::spawn(move || {
            first_poll_completed.wait();

            sender.send(42);
        });

        let receive_thread = thread::spawn(move || {
            let mut receiver = Box::pin(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            first_poll_completed_clone.wait();

            // We do not know how many polls this will take, so we switch into real async.
            block_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}

#[test]
fn placed_send_receive_unbiased_mt() {
    with_watchdog(|| {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
        // its storage stays allocated, writable and at a stable address until the end of
        // the test, which outlives both endpoints.
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            block_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}

#[test]
fn placed_drop_receive_unbiased_mt() {
    with_watchdog(|| {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
        // its storage stays allocated, writable and at a stable address until the end of
        // the test, which outlives both endpoints.
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            block_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Err(Disconnected)));
            });
        });

        let send_thread = thread::spawn(move || {
            drop(sender);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}

#[test]
fn placed_drop_send_unbiased_mt() {
    with_watchdog(|| {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        // SAFETY: `place` is a fresh container that holds no other event, box-pinned so
        // its storage stays allocated, writable and at a stable address until the end of
        // the test, which outlives both endpoints.
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            drop(receiver);
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    });
}
