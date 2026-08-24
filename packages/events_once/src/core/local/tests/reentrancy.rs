use super::*;

// Regression test for the synchronous reentrancy hazard in `set`. The wake callback that
// `set` fires may poll the receiver synchronously, and it must find the event fully
// published: the value stored, the terminal state committed and the awaiter slot emptied.
// The local variant reaches that state with a single state write and never publishes the
// signaling state that the thread-safe variant uses (see `state.rs`), so the callback must
// observe `EVENT_SET` and read out the value. Ref: docs/callback-safety.md.
#[test]
fn boxed_send_with_reentrant_waker_observes_set() {
    type ObservedResult = Poll<Result<i32, Disconnected>>;

    with_watchdog(|| {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let receiver_holder: Rc<RefCell<Option<Pin<Box<_>>>>> =
            Rc::new(RefCell::new(Some(Box::pin(receiver))));
        let receiver_for_waker = Rc::clone(&receiver_holder);

        let reentrant_observed: Rc<RefCell<Option<ObservedResult>>> = Rc::new(RefCell::new(None));
        let observed_for_waker = Rc::clone(&reentrant_observed);

        // SAFETY: The action is not `Send`, and this test keeps every waker on one thread.
        let (waker, was_woken) = unsafe {
            wake_action_waker(move || {
                // Synchronously poll the receiver from inside the waker.
                // The receiver should observe EVENT_SET and return Ready(Ok(42)).
                let mut holder = receiver_for_waker.borrow_mut();
                let receiver = holder.as_mut().expect("receiver still held");
                let noop = Waker::noop();
                let mut cx = task::Context::from_waker(noop);
                let result = receiver.as_mut().poll(&mut cx);
                *observed_for_waker.borrow_mut() = Some(result);
            })
        };

        // First poll transitions BOUND -> AWAITING and stores the
        // reentrant waker.
        {
            let mut holder = receiver_holder.borrow_mut();
            let receiver = holder.as_mut().expect("receiver still held");
            let mut cx = task::Context::from_waker(&waker);
            assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
        }

        // Send a value. This calls `set` which transitions AWAITING -> SET
        // and then invokes the waker, which must observe the SET state
        // and consume the value reentrantly.
        sender.send(42);

        assert!(was_woken.get());
        let observed = reentrant_observed.borrow_mut().take();
        assert!(
            matches!(observed, Some(Poll::Ready(Ok(42)))),
            "reentrant poll should observe SET and read the value",
        );

        // The receiver was consumed reentrantly; the receiver_holder still
        // owns the Pin<Box> shell, drop it to release.
        drop(receiver_holder.borrow_mut().take());
    });
}

// Parity counterpart of the `set` case above. A waker fired by the sender
// drop that synchronously polls the receiver must likewise observe a
// terminal state, here DISCONNECTED.
#[test]
fn boxed_sender_drop_with_reentrant_waker_observes_disconnected() {
    type ObservedResult = Poll<Result<i32, Disconnected>>;

    with_watchdog(|| {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let receiver_holder: Rc<RefCell<Option<Pin<Box<_>>>>> =
            Rc::new(RefCell::new(Some(Box::pin(receiver))));
        let receiver_for_waker = Rc::clone(&receiver_holder);

        let reentrant_observed: Rc<RefCell<Option<ObservedResult>>> = Rc::new(RefCell::new(None));
        let observed_for_waker = Rc::clone(&reentrant_observed);

        // SAFETY: The action is not `Send`, and this test keeps every waker on one thread.
        let (waker, was_woken) = unsafe {
            wake_action_waker(move || {
                let mut holder = receiver_for_waker.borrow_mut();
                let receiver = holder.as_mut().expect("receiver still held");
                let noop = Waker::noop();
                let mut cx = task::Context::from_waker(noop);
                let result = receiver.as_mut().poll(&mut cx);
                *observed_for_waker.borrow_mut() = Some(result);
            })
        };

        // First poll transitions BOUND -> AWAITING and stores the
        // reentrant waker.
        {
            let mut holder = receiver_holder.borrow_mut();
            let receiver = holder.as_mut().expect("receiver still held");
            let mut cx = task::Context::from_waker(&waker);
            assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
        }

        // Dropping the sender transitions AWAITING -> DISCONNECTED and then
        // invokes the waker, which must observe the terminal state.
        drop(sender);

        assert!(was_woken.get());
        let observed = reentrant_observed.borrow_mut().take();
        assert!(
            matches!(observed, Some(Poll::Ready(Err(Disconnected)))),
            "reentrant poll should observe DISCONNECTED",
        );

        drop(receiver_holder.borrow_mut().take());
    });
}

// Regression test for cancellation through a reentrant waker destructor. `cancel` extracts the
// stored waker and publishes DISCONNECTED before deferring its destruction. The waker destructor
// drops the sender, which observes the disconnection and performs the sole cleanup. Ref:
// docs/callback-safety.md. This runs under Miri so an ordering regression that accesses released
// storage is detected.
#[test]
fn boxed_receiver_cancel_with_sender_dropping_waker_preserves_storage() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let (data, sender_dropped) = DropOnWakerRelease::new(sender);
    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let waker = unsafe { drop_waker(data) };

    // First poll transitions BOUND -> AWAITING and stores a clone of the
    // waker inside the event.
    let mut cx = task::Context::from_waker(&waker);
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    // Drop our local waker so only the event's stored clone remains; the
    // sender is still owned behind that clone.
    drop(waker);
    assert!(!sender_dropped.get());

    // Dropping the receiver publishes DISCONNECTED before releasing the stored waker. Its
    // destructor drops the sender, which observes the disconnection and performs the sole cleanup.
    drop(receiver);

    assert!(sender_dropped.get(), "{REENTRANCY_REQUIRED}");
}

// Regression test for the reentrancy hazard in `set`. The wake callback
// fired by the sender is free to drop the receiver, which completes the
// event and releases its storage while `set` is still on the stack. `set`
// must therefore reach the event through an `UnsafeCell` and must not touch
// it after waking. Ref: docs/callback-safety.md. Runs under Miri so the
// protected-pointer deallocation is caught if the shape regresses.
#[test]
fn boxed_send_with_reentrant_receiver_drop_releases_storage() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();

    let (data, receiver_dropped) = DropOnWakerRelease::new(Box::pin(receiver));
    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let waker = unsafe { drop_waker(Arc::clone(&data)) };

    // First poll transitions BOUND -> AWAITING and stores a clone of the
    // waker inside the event.
    data.with_value(|receiver| {
        let mut cx = task::Context::from_waker(&waker);
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
    });

    // Leave the event's stored clone as the only reference, so waking it
    // runs the reentrant drop.
    drop(waker);
    drop(data);
    assert!(!receiver_dropped.get());

    // `set` stores the value, transitions AWAITING -> SET and wakes, which
    // drops the receiver, which consumes the value and frees the event.
    sender.send(42);

    assert!(receiver_dropped.get(), "{REENTRANCY_REQUIRED}");
}

// Regression test for the reentrancy hazard in `sender_dropped_without_set`.
// Identical in shape to the `set` case above, except that the receiver
// observes DISCONNECTED instead of SET.
#[test]
fn boxed_sender_drop_with_reentrant_receiver_drop_releases_storage() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();

    let (data, receiver_dropped) = DropOnWakerRelease::new(Box::pin(receiver));
    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let waker = unsafe { drop_waker(Arc::clone(&data)) };

    data.with_value(|receiver| {
        let mut cx = task::Context::from_waker(&waker);
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
    });

    drop(waker);
    drop(data);
    assert!(!receiver_dropped.get());

    // `sender_dropped_without_set` transitions AWAITING -> DISCONNECTED and
    // wakes, which drops the receiver, which frees the event.
    drop(sender);

    assert!(receiver_dropped.get(), "{REENTRANCY_REQUIRED}");
}

/// Explains a failure to reach the reentrant drop that a regression test exists to exercise.
/// Without it the test would pass no matter how the code under test behaved.
const REENTRANCY_REQUIRED: &str =
    "the event must have held a waker clone whose drop reentered the operation under test";

/// Explains a failure to reach the reentrant clone that a regression test exists to exercise.
/// Without it the test would pass no matter how the code under test behaved.
const WAKER_CLONE_REQUIRED: &str =
    "the poll must have cloned the waker it was given, which is what re-enters the sender";

// Regression tests for the reentrancy hazard in `poll`. Registering an awaiter clones the
// waker, which is user code that may operate on the sender endpoint of the same event and
// move it into a terminal state. The poll must observe the state the clone left behind
// instead of overwriting it with EVENT_AWAITING, which would strand the receiver and lose
// the result. Ref: docs/callback-safety.md.
#[test]
fn boxed_poll_with_reentrant_send_during_waker_clone_observes_set() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker(move || sender.send(42)) };

    let mut cx = task::Context::from_waker(&waker);
    let poll_result = receiver.as_mut().poll(&mut cx);

    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_poll_with_reentrant_sender_drop_during_waker_clone_observes_disconnected() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker(move || drop(sender)) };

    let mut cx = task::Context::from_waker(&waker);
    let poll_result = receiver.as_mut().poll(&mut cx);

    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

// The same hazard on the re-poll path, where the reentrant sender additionally consumes the
// previously registered waker on its way to the terminal state.
#[test]
fn boxed_repoll_with_reentrant_send_during_waker_clone_observes_set() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    // First poll transitions BOUND -> AWAITING and registers a waker for the sender to take.
    let mut cx = task::Context::from_waker(Waker::noop());
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker(move || sender.send(42)) };

    let mut cx = task::Context::from_waker(&waker);
    let poll_result = receiver.as_mut().poll(&mut cx);

    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
    assert!(matches!(poll_result, Poll::Ready(Ok(42))));
}

#[test]
fn boxed_repoll_with_reentrant_sender_drop_during_waker_clone_observes_disconnected() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let mut cx = task::Context::from_waker(Waker::noop());
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker(move || drop(sender)) };

    let mut cx = task::Context::from_waker(&waker);
    let poll_result = receiver.as_mut().poll(&mut cx);

    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
    assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
}

// The re-poll path releases the registration made by the previous poll, which is user code
// that may drop the sender and thereby complete the event from inside the re-poll. The
// replacement registration is written before that destructor runs, so the completion finds it
// and wakes it. Reporting pending therefore loses no wakeup.
// Ref: docs/callback-safety.md.
#[test]
fn boxed_repoll_with_reentrant_sender_drop_during_previous_waker_release_wakes_new_waker() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let (data, sender_dropped) = DropOnWakerRelease::new(sender);
    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let previous_waker = unsafe { drop_waker(data) };

    // First poll transitions BOUND -> AWAITING and registers a clone of `previous_waker`.
    let mut cx = task::Context::from_waker(&previous_waker);
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    // Release our own reference so the registration inside the event is the last one and
    // dropping it drops the sender it carries.
    drop(previous_waker);
    assert!(!sender_dropped.get());

    // SAFETY: The action is not `Send`, and this test keeps every waker on one thread.
    let (new_waker, new_waker_was_woken) = unsafe { wake_action_waker(|| {}) };

    let mut cx = task::Context::from_waker(&new_waker);
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    assert!(sender_dropped.get(), "{REENTRANCY_REQUIRED}");
    assert!(
        new_waker_was_woken.get(),
        "the completion must have reached the registration made by this poll"
    );

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Err(Disconnected))
    ));
}

#[test]
fn boxed_send_survives_waker_wake_panic() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker_panicking_on_clone_release(|| {}) };

    let mut cx = task::Context::from_waker(&waker);
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
    drop(waker);

    assert_panics(|| sender.send(42));

    let mut cx = task::Context::from_waker(Waker::noop());
    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));
}

#[test]
fn boxed_repoll_survives_previous_waker_drop_panic() {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) = unsafe { clone_action_waker_panicking_on_clone_release(|| {}) };

    let mut cx = task::Context::from_waker(&waker);
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
    drop(waker);

    assert_panics(|| {
        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);
    });

    let mut cx = task::Context::from_waker(Waker::noop());
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    sender.send(42);

    assert!(matches!(
        receiver.as_mut().poll(&mut cx),
        Poll::Ready(Ok(42))
    ));
}

/// Counts its own drops, so a test can tell a value that was delivered exactly once apart from
/// one that was extracted twice or lost.
struct DropCounter {
    drops: Rc<Cell<usize>>,
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.drops.set(self.drops.get().saturating_add(1));
    }
}

// Releasing the waker clone that a reentrant send made useless is user code, so it may unwind.
// The value must still be inside the event at that moment: a poll that extracts the value
// first leaves EVENT_SET backed by an uninitialized cell, which the receiver's own cleanup
// then reads a second time. Ref: docs/callback-safety.md.
#[test]
fn boxed_poll_unwinding_during_waker_clone_release_leaves_value_in_event() {
    let drops = Rc::new(Cell::new(0_usize));

    let (sender, receiver) = LocalEvent::<DropCounter>::boxed();
    let mut receiver = Box::pin(receiver);

    let value = DropCounter {
        drops: Rc::clone(&drops),
    };

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) =
        unsafe { clone_action_waker_panicking_on_clone_release(move || sender.send(value)) };

    let mut cx = task::Context::from_waker(&waker);
    assert_panics_with(
        || receiver.as_mut().poll(&mut cx),
        |message| assert!(message.contains("waker clone release")),
    );

    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");

    if drops.get() != 0 {
        // The poll extracted the value before running the callback, so the event now claims a
        // value it no longer holds. Letting the receiver clean up would read that cell again,
        // so leak the event instead of escalating the failure into undefined behavior.
        mem::forget(receiver);

        panic!("the value must stay in the event while a callback can still unwind past it");
    }

    // The event still owns the value, so the receiver's cleanup delivers exactly one drop.
    drop(receiver);

    assert_eq!(drops.get(), 1);
}

// The same hazard on the re-poll path, which releases the useless clone through the same arm.
#[test]
fn boxed_repoll_unwinding_during_waker_clone_release_leaves_value_in_event() {
    let drops = Rc::new(Cell::new(0_usize));

    let (sender, receiver) = LocalEvent::<DropCounter>::boxed();
    let mut receiver = Box::pin(receiver);

    // First poll transitions BOUND -> AWAITING and registers a waker for the sender to take.
    let mut cx = task::Context::from_waker(Waker::noop());
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    let value = DropCounter {
        drops: Rc::clone(&drops),
    };

    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let (waker, cloned) =
        unsafe { clone_action_waker_panicking_on_clone_release(move || sender.send(value)) };

    let mut cx = task::Context::from_waker(&waker);
    assert_panics_with(
        || receiver.as_mut().poll(&mut cx),
        |message| assert!(message.contains("waker clone release")),
    );

    assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");

    if drops.get() != 0 {
        mem::forget(receiver);

        panic!("the value must stay in the event while a callback can still unwind past it");
    }

    drop(receiver);

    assert_eq!(drops.get(), 1);
}
