use super::*;

// Regression test for the synchronous reentrancy hazard in
// `sender_dropped_without_set`. A waker fired by the sender drop that
// synchronously polls the receiver must observe a terminal state
// (DISCONNECTED), not the transient SIGNALING state — otherwise the
// reentrant poll would spin in `poll_signaling` while the sender is
// blocked inside `wake()`, producing a same-thread deadlock.
#[test]
fn boxed_sender_drop_with_reentrant_waker_does_not_deadlock() {
    type ObservedResult = Poll<Result<i32, Disconnected>>;

    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();
        let receiver_holder: Rc<RefCell<Option<Pin<Box<_>>>>> =
            Rc::new(RefCell::new(Some(Box::pin(receiver))));
        let receiver_for_waker = Rc::clone(&receiver_holder);

        let reentrant_observed: Rc<RefCell<Option<ObservedResult>>> = Rc::new(RefCell::new(None));
        let observed_for_waker = Rc::clone(&reentrant_observed);

        // SAFETY: The action is not `Send`, and this test keeps every waker on one thread.
        let (waker, was_woken) = unsafe {
            wake_action_waker(move || {
                // Synchronously poll the receiver from inside the waker.
                // With the buggy ordering this would enter `poll_signaling`
                // and spin while we are still blocked inside `wake()`.
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

        // Drop the sender. This calls `sender_dropped_without_set`,
        // which transitions AWAITING -> SIGNALING, then must
        // transition to DISCONNECTED before invoking the waker so
        // that the reentrant poll observes a terminal state.
        drop(sender);

        assert!(was_woken.get());
        let observed = reentrant_observed.borrow_mut().take();
        assert!(
            matches!(observed, Some(Poll::Ready(Err(Disconnected)))),
            "reentrant poll should observe DISCONNECTED",
        );

        // Drop the receiver to release its half of the event.
        drop(receiver_holder.borrow_mut().take());
    });
}

// Parity counterpart of the disconnect case above. A waker fired by `send` that
// synchronously polls the receiver must likewise observe a terminal state, here SET, and read
// out the value.
#[test]
fn boxed_send_with_reentrant_waker_observes_set() {
    type ObservedResult = Poll<Result<i32, Disconnected>>;

    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();
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

        // First poll transitions BOUND -> AWAITING and stores the reentrant waker.
        {
            let mut holder = receiver_holder.borrow_mut();
            let receiver = holder.as_mut().expect("receiver still held");
            let mut cx = task::Context::from_waker(&waker);
            assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
        }

        // `set` reaches a terminal state and then invokes the waker, which must observe SET
        // and consume the value reentrantly.
        sender.send(42);

        assert!(was_woken.get());
        let observed = reentrant_observed.borrow_mut().take();
        assert!(
            matches!(observed, Some(Poll::Ready(Ok(42)))),
            "reentrant poll should observe SET and read the value",
        );

        // The receiver was consumed reentrantly; drop the shell that still owns it.
        drop(receiver_holder.borrow_mut().take());
    });
}

#[test]
fn boxed_send_survives_waker_wake_panic() {
    let (sender, receiver) = Event::<i32>::boxed();
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
    let (sender, receiver) = Event::<i32>::boxed();
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

// Parity counterparts of the `LocalEvent` reentrancy regression tests in `core/local.rs`.
// A wake callback fired while completing or cancelling an event is free to drop the receiver,
// which releases the event storage while the operation is still on the stack. Ref:
// docs/callback-safety.md. These run under Miri, which is what detects a regression here.
#[test]
fn boxed_receiver_cancel_with_sender_dropping_waker_preserves_storage() {
    let (sender, receiver) = Event::<i32>::boxed();
    let mut receiver = Box::pin(receiver);

    let (data, sender_dropped) = DropOnWakerRelease::new(sender);
    // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
    let waker = unsafe { drop_waker(data) };

    // First poll transitions BOUND -> AWAITING and stores a clone of the waker in the event.
    let mut cx = task::Context::from_waker(&waker);
    assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

    // Drop our local waker so only the event's stored clone remains; the sender is still
    // owned behind that clone.
    drop(waker);
    assert!(!sender_dropped.get());

    // Dropping the receiver cancels the wait. `cancel` extracts the stored waker and publishes
    // DISCONNECTED before deferring its destruction. The waker destructor drops the sender, which
    // observes the disconnection and performs the sole cleanup.
    drop(receiver);

    assert!(sender_dropped.get(), "{REENTRANCY_REQUIRED}");
}

/// Owns a receiver on behalf of a waker payload and drops it only once the event has reached
/// a terminal state.
///
/// The reentrancy tests below release this from inside a sender operation, where dropping the
/// receiver re-enters `Event::cancel`. That call spins while the event is in the
/// transient `EVENT_SIGNALING` state, and the sender that would leave that state is the very
/// operation blocked in this destructor - so a regression in terminal-state publication would
/// hang the test instead of failing it. Checking readiness first turns that regression into a
/// deterministic failure, which is what mutation testing needs given that watchdogs are
/// disabled there. Ref: docs/testing.md, "Tests must not hang".
struct TerminalStateReceiverDrop {
    receiver: Option<Pin<Box<BoxedReceiver<i32>>>>,
}

impl TerminalStateReceiverDrop {
    fn new(receiver: BoxedReceiver<i32>) -> Self {
        Self {
            receiver: Some(Box::pin(receiver)),
        }
    }

    fn receiver_mut(&mut self) -> Pin<&mut BoxedReceiver<i32>> {
        self.receiver
            .as_mut()
            .expect("the receiver is only taken while dropping")
            .as_mut()
    }
}

impl Drop for TerminalStateReceiverDrop {
    fn drop(&mut self) {
        let receiver = self
            .receiver
            .take()
            .expect("the receiver is only taken while dropping");

        if receiver.is_ready() {
            drop(receiver);
            return;
        }

        // The event has not published a terminal state, so dropping the receiver here would
        // spin forever waiting for the sender that is blocked in this destructor. We leak the
        // receiver instead and report the defect.
        mem::forget(receiver);

        assert!(
            thread::panicking(),
            "the sender must publish a terminal state before releasing the stored waker"
        );
    }
}

#[test]
fn boxed_send_with_reentrant_receiver_drop_releases_storage() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();

        let (data, receiver_dropped) =
            DropOnWakerRelease::new(TerminalStateReceiverDrop::new(receiver));
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let waker = unsafe { drop_waker(Arc::clone(&data)) };

        // First poll transitions BOUND -> AWAITING and stores a clone of the waker in the
        // event.
        data.with_value(|holder| {
            let mut cx = task::Context::from_waker(&waker);
            assert!(matches!(holder.receiver_mut().poll(&mut cx), Poll::Pending));
        });

        // Leave the event's stored clone as the only reference, so waking it runs the
        // reentrant drop.
        drop(waker);
        drop(data);
        assert!(!receiver_dropped.get());

        // `set` stores the value, reaches a terminal state and wakes, which drops the
        // receiver, which consumes the value and frees the event.
        sender.send(42);

        assert!(receiver_dropped.get(), "{REENTRANCY_REQUIRED}");
    });
}

#[test]
fn boxed_sender_drop_with_reentrant_receiver_drop_releases_storage() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();

        let (data, receiver_dropped) =
            DropOnWakerRelease::new(TerminalStateReceiverDrop::new(receiver));
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let waker = unsafe { drop_waker(Arc::clone(&data)) };

        data.with_value(|holder| {
            let mut cx = task::Context::from_waker(&waker);
            assert!(matches!(holder.receiver_mut().poll(&mut cx), Poll::Pending));
        });

        drop(waker);
        drop(data);
        assert!(!receiver_dropped.get());

        // `sender_dropped_without_set` reaches DISCONNECTED and wakes, which drops the
        // receiver, which frees the event.
        drop(sender);

        assert!(receiver_dropped.get(), "{REENTRANCY_REQUIRED}");
    });
}

// The re-poll path destroys the registration made by the previous poll before it registers
// the replacement. Releasing that registration is user code that may drop the sender, which
// completes the event from inside the re-poll: the sender observes EVENT_BOUND and publishes
// EVENT_DISCONNECTED without touching the awaiter, so the re-poll must observe that state,
// release the replacement it made and report the disconnect.
// Ref: docs/callback-safety.md.
#[test]
fn boxed_repoll_with_reentrant_sender_drop_during_previous_waker_release_observes_disconnected() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let (data, sender_dropped) = DropOnWakerRelease::new(sender);
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let previous_waker = unsafe { drop_waker(data) };

        // First poll transitions BOUND -> AWAITING and registers a clone of `previous_waker`.
        let mut cx = task::Context::from_waker(&previous_waker);
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // Release our own reference so that the registration inside the event is the last
        // one, and destroying it drops the sender it carries.
        drop(previous_waker);
        assert!(!sender_dropped.get());

        let (replacement_data, replacement_released) = DropOnWakerRelease::new(());
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let replacement_waker = unsafe { drop_waker(Arc::clone(&replacement_data)) };

        let mut cx = task::Context::from_waker(&replacement_waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(sender_dropped.get(), "{REENTRANCY_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));

        // The event completed, so it must not have kept the replacement registration - once
        // we release our own references, the payload is dropped.
        drop(replacement_waker);
        drop(replacement_data);
        assert!(
            replacement_released.get(),
            "the re-poll must release the replacement registration it made"
        );
    });
}

/// Explains a failure to reach the reentrant drop that a regression test exists to exercise.
/// Without it the test would pass no matter how the code under test behaved.
const REENTRANCY_REQUIRED: &str =
    "the event must have held a waker clone whose drop reentered the operation under test";

/// Explains a failure to reach the reentrant clone that a regression test exists to exercise.
/// Without it the test would pass no matter how the code under test behaved.
const WAKER_CLONE_REQUIRED: &str =
    "the poll must have cloned the waker it was given, which is what re-enters the sender";

// Parity counterparts of the `LocalEvent` waker-clone reentrancy regression tests in
// `core/local.rs`. Registering an awaiter clones the waker, which is user code that may
// operate on the sender endpoint of the same event and move it into a terminal state. The
// poll must observe the state the clone left behind. Ref: docs/callback-safety.md.
#[test]
fn boxed_poll_with_reentrant_send_during_waker_clone_observes_set() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || sender.send(42)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    });
}

#[test]
fn boxed_poll_with_reentrant_sender_drop_during_waker_clone_observes_disconnected() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || drop(sender)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    });
}

#[test]
fn boxed_repoll_with_reentrant_send_during_waker_clone_observes_set() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        // First poll transitions BOUND -> AWAITING and registers a waker for the sender to
        // take.
        let mut cx = task::Context::from_waker(Waker::noop());
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || sender.send(42)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    });
}

#[test]
fn boxed_repoll_with_reentrant_sender_drop_during_waker_clone_observes_disconnected() {
    with_watchdog(|| {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || drop(sender)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    });
}

/// Counts its own drops, so a test can tell a value that was delivered exactly once apart from
/// one that was extracted twice or lost.
struct DropCounter {
    drops: Arc<atomic::AtomicUsize>,
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.drops.fetch_add(1, atomic::Ordering::Relaxed);
    }
}

// Parity counterpart of the local unwinding test. Releasing the awaiter that a racing send
// made useless is user code, so it may unwind. The value must still be inside the event at
// that moment, or the receiver's own cleanup reads an extracted cell a second time.
// Ref: docs/callback-safety.md.
#[test]
fn boxed_poll_unwinding_during_waker_clone_release_leaves_value_in_event() {
    with_watchdog(|| {
        let drops = Arc::new(atomic::AtomicUsize::new(0));

        let (sender, receiver) = Event::<DropCounter>::boxed();
        let mut receiver = Box::pin(receiver);

        let value = DropCounter {
            drops: Arc::clone(&drops),
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

        if drops.load(atomic::Ordering::Relaxed) != 0 {
            // The poll extracted the value before running the callback, so the event now
            // claims a value it no longer holds. Letting the receiver clean up would read
            // that cell again, so leak the event instead of escalating the failure into
            // undefined behavior.
            mem::forget(receiver);

            panic!("the value must stay in the event while a callback can still unwind past it");
        }

        // The event still owns the value, so the receiver's cleanup delivers exactly one drop.
        drop(receiver);

        assert_eq!(drops.load(atomic::Ordering::Relaxed), 1);
    });
}
