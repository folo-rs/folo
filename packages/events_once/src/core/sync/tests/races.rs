use super::*;

/// Installs a hook closure and runs the test body while holding the
/// serialization mutex. The hook is always removed on exit.
fn with_hook(hook: &Mutex<Option<Arc<HookFn>>>, closure: Arc<HookFn>, body: impl FnOnce()) {
    let guard = HOOK_SERIALIZATION_MUTEX.lock().unwrap();
    *hook.lock().unwrap() = Some(closure);

    // We catch panics from the test body so that we can clean up the hook and
    // drop the serialization guard while not panicking, preventing mutex poisoning.
    let result = catch_unwind(AssertUnwindSafe(body));

    *hook.lock().unwrap() = None;
    drop(guard);

    if let Err(payload) = result {
        resume_unwind(payload);
    }
}

struct BarrierHook {
    /// Signaled when the hook fires, indicating that the hooked code
    /// has reached the synchronization point.
    entered: Arc<Barrier>,

    /// Waited on before the hook returns, giving the test thread a
    /// window to perform a racing operation.
    proceed: Arc<Barrier>,

    /// The closure to install as a hook.
    hook: Arc<HookFn>,
}

/// Creates a two-barrier hook closure that pauses execution at a
/// synchronization point, giving the test thread a window to perform
/// a racing operation.
fn barrier_hook() -> BarrierHook {
    let entered = Arc::new(Barrier::new(2));
    let proceed = Arc::new(Barrier::new(2));
    let e = Arc::clone(&entered);
    let p = Arc::clone(&proceed);
    let hook: Arc<HookFn> = Arc::new(move || {
        e.wait();
        p.wait();
    });
    BarrierHook {
        entered,
        proceed,
        hook,
    }
}

#[test]
fn boxed_poll_bound_races_sender_disconnect() {
    with_watchdog(|| {
        let BarrierHook {
            entered,
            proceed,
            hook,
        } = barrier_hook();
        with_hook(&HOOK_POLL_BOUND_PRE_CAS, hook, || {
            let (sender, receiver) = Event::<i32>::boxed();

            // Receiver polls on a separate thread. It will write the
            // waker and then pause at the hook, before the CAS.
            let receive_thread = thread::spawn(move || {
                HOOK_PARTICIPANT.set(true);
                let mut receiver = Box::pin(receiver);
                let mut cx = task::Context::from_waker(Waker::noop());
                receiver.as_mut().poll(&mut cx)
            });

            // Wait for the hook to fire (receiver wrote waker).
            entered.wait();

            // Drop the sender while the receiver is paused. This
            // transitions BOUND -> SIGNALING -> DISCONNECTED.
            drop(sender);

            // Release the receiver so its CAS(BOUND->AWAITING) fails
            // with DISCONNECTED.
            proceed.wait();

            let poll_result = receive_thread.join().unwrap();
            assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
        });
    });
}

#[test]
fn boxed_poll_awaiting_races_sender_set() {
    with_watchdog(|| {
        let BarrierHook {
            entered,
            proceed,
            hook,
        } = barrier_hook();
        with_hook(&HOOK_POLL_AWAITING_PRE_CAS, hook, || {
            let (sender, receiver) = Event::<i32>::boxed();
            let mut receiver = Box::pin(receiver);

            // First poll transitions BOUND -> AWAITING.
            let mut cx = task::Context::from_waker(Waker::noop());
            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // Second poll on a separate thread enters poll_awaiting
            // and pauses at the hook before the CAS.
            let receive_thread = thread::spawn(move || {
                HOOK_PARTICIPANT.set(true);
                let mut cx = task::Context::from_waker(Waker::noop());
                receiver.as_mut().poll(&mut cx)
            });

            // Wait for the hook to fire.
            entered.wait();

            // Send value while the receiver is paused. This
            // transitions AWAITING -> SIGNALING -> SET.
            sender.send(42);

            // Release the receiver so its CAS(AWAITING->BOUND) fails
            // with SET.
            proceed.wait();

            let poll_result = receive_thread.join().unwrap();
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        });
    });
}

#[test]
fn boxed_poll_awaiting_races_sender_disconnect() {
    with_watchdog(|| {
        let BarrierHook {
            entered,
            proceed,
            hook,
        } = barrier_hook();
        with_hook(&HOOK_POLL_AWAITING_PRE_CAS, hook, || {
            let (sender, receiver) = Event::<i32>::boxed();
            let mut receiver = Box::pin(receiver);

            // First poll transitions BOUND -> AWAITING.
            let mut cx = task::Context::from_waker(Waker::noop());
            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // Second poll on a separate thread enters poll_awaiting
            // and pauses at the hook before the CAS.
            let receive_thread = thread::spawn(move || {
                HOOK_PARTICIPANT.set(true);
                let mut cx = task::Context::from_waker(Waker::noop());
                receiver.as_mut().poll(&mut cx)
            });

            // Wait for the hook to fire.
            entered.wait();

            // Drop sender while the receiver is paused. This
            // transitions AWAITING -> SIGNALING -> DISCONNECTED.
            drop(sender);

            // Release the receiver so its CAS(AWAITING->BOUND) fails
            // with DISCONNECTED.
            proceed.wait();

            let poll_result = receive_thread.join().unwrap();
            assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
        });
    });
}

// This test verifies that `cancel` correctly handles the case where the sender is
// mid-SIGNALING when the receiver is dropped. The cancellation CAS loop
// that spins on SIGNALING, so the receiver waits for the sender to finish before writing
// DISCONNECTED. The receiver-drop must happen on a separate thread because `cancel`
// will spin until the sender completes, and the sender is blocked on the hook barrier.
#[test]
// The receiver's `cancel` spins while the event is in SIGNALING, and here the sender only
// leaves SIGNALING once the test releases its barrier. Miri's interpreter makes such a
// cross-thread spin extremely slow, so this scenario is reserved for native runs.
#[cfg_attr(miri, ignore)]
fn boxed_cancel_races_sender_signaling() {
    with_watchdog(|| {
        let BarrierHook {
            entered,
            proceed,
            hook,
        } = barrier_hook();
        with_hook(&HOOK_SET_IN_SIGNALING, hook, || {
            let (sender, receiver) = Event::<i32>::boxed();
            let mut receiver = Box::pin(receiver);

            // First poll transitions BOUND -> AWAITING.
            let mut cx = task::Context::from_waker(Waker::noop());
            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // Sender sends on a separate thread. The fetch_add
            // transitions AWAITING -> SIGNALING. The sender then
            // pauses at the hook in the SIGNALING state.
            let send_thread = thread::spawn(move || {
                HOOK_PARTICIPANT.set(true);
                sender.send(42);
            });

            // Wait for the hook to fire (sender is in SIGNALING).
            entered.wait();

            // Drop the receiver on a separate thread. cancel
            // will spin on SIGNALING until the sender completes
            // its transition, so we cannot block this thread — we
            // need it to release the sender via proceed.wait().
            let drop_thread = thread::spawn(move || {
                drop(receiver);
            });

            // Release the sender so it can complete its transition
            // from SIGNALING -> SET. This unblocks cancel's
            // spin, which then sees SET and reads the value.
            proceed.wait();

            send_thread.join().unwrap();
            drop_thread.join().unwrap();
        });
    });
}

// Regression test for issue #462: `is_ready()` must not report readiness while the
// sender is mid-`set()` in the transient SIGNALING state, because `into_value()` still
// reports `Pending` there. The two must agree. `EVENT_SIGNALING` is not a terminal
// state, so it must count as "not ready".
#[test]
fn boxed_is_ready_false_while_sender_signaling() {
    with_watchdog(|| {
        let BarrierHook {
            entered,
            proceed,
            hook,
        } = barrier_hook();
        with_hook(&HOOK_SET_IN_SIGNALING, hook, || {
            let (sender, receiver) = Event::<i32>::boxed();
            let mut receiver = Box::pin(receiver);

            // First poll transitions BOUND -> AWAITING so that the sender's
            // `set()` takes the AWAITING path that pauses at the hook.
            let mut cx = task::Context::from_waker(Waker::noop());
            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // Sender sends on a separate thread. The `fetch_add` in `set` transitions
            // AWAITING -> SIGNALING, then the sender pauses at the hook while still in the
            // SIGNALING state. (The sender-disconnect path reaches SIGNALING via `swap`
            // instead; see `sender_dropped_without_set`.)
            let send_thread = thread::spawn(move || {
                HOOK_PARTICIPANT.set(true);
                sender.send(42);
            });

            // Wait for the hook to fire (sender is parked in SIGNALING).
            entered.wait();

            // The value is not yet retrievable in SIGNALING, so readiness must be
            // false here, consistent with `into_value()` reporting `Pending`.
            assert!(!receiver.is_ready());

            // Release the sender so it completes SIGNALING -> SET.
            proceed.wait();
            send_thread.join().unwrap();

            // Now in the terminal SET state, readiness agrees with retrieval.
            assert!(receiver.is_ready());
            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        });
    });
}

// Synchronous extraction has a concurrency boundary that `Future::poll` does not cover: it
// classifies the observed state and, for a terminal state, finalizes the event and may
// release its storage while the sender is still acting. The transient SIGNALING state is the
// interesting classification, because there the sender owns the event fields but has not
// published an outcome yet - an extraction must report pending and hand back a usable
// receiver. The hook that parks a sender in SIGNALING is private to this module, which is why
// this test lives here rather than beside the other `into_value` tests in `sync_receiver.rs`.
// Ref: docs/testing.md, "Testing atomic operations and custom synchronization".
#[test]
fn boxed_into_value_pending_while_sender_signaling() {
    with_watchdog(|| {
        let BarrierHook {
            entered,
            proceed,
            hook,
        } = barrier_hook();
        with_hook(&HOOK_SET_IN_SIGNALING, hook, || {
            let (sender, mut receiver) = Event::<i32>::boxed();

            // First poll transitions BOUND -> AWAITING, which is what routes the sender's
            // `set` through SIGNALING.
            let mut cx = task::Context::from_waker(Waker::noop());
            assert!(matches!(
                Pin::new(&mut receiver).poll(&mut cx),
                Poll::Pending
            ));

            let send_thread = thread::spawn(move || {
                HOOK_PARTICIPANT.set(true);
                sender.send(42);
            });

            // Wait for the hook to fire (sender is parked in SIGNALING, holding the awaiter).
            entered.wait();

            if receiver.is_ready() {
                // Consuming or dropping the receiver now would finalize the event, which
                // spins until the sender leaves SIGNALING - and only this thread can release
                // the sender. We leak the receiver, release the sender and fail, so that a
                // regression is reported instead of hanging when no watchdog is active.
                mem::forget(receiver);
                proceed.wait();
                send_thread.join().unwrap();

                panic!("SIGNALING must not be reported as a completed event");
            }

            let extraction = receiver.into_value();

            // Release the sender before asserting, so that a failure cannot leave it parked
            // in the hook.
            proceed.wait();
            send_thread.join().unwrap();

            let Err(IntoValueError::Pending(receiver)) = extraction else {
                panic!("the value must not be extractable while the sender is signaling");
            };

            // The retained receiver is still connected to the event, so it observes the value
            // that the sender has since published.
            assert_eq!(
                receiver.into_value().expect("the sender published a value"),
                42
            );
        });
    });
}
