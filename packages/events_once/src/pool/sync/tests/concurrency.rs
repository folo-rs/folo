use super::*;

#[test]
fn send_receive_mt() {
    with_watchdog(|| {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

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
fn receive_send_receive_mt() {
    with_watchdog(|| {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

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
fn send_receive_unbiased_mt() {
    with_watchdog(|| {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

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
fn drop_receive_unbiased_mt() {
    with_watchdog(|| {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

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
fn drop_send_unbiased_mt() {
    with_watchdog(|| {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

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
fn drop_pool_send_receive() {
    let pool = EventPool::<i32>::new();

    let (sender, receiver) = pool.rent();

    drop(pool);

    let mut receiver = Box::pin(receiver);

    sender.send(42);

    let mut cx = task::Context::from_waker(Waker::noop());

    let poll_result = receiver.as_mut().poll(&mut cx);
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}
