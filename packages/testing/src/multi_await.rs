use parking_lot::Mutex;
use std::any::type_name;
use std::fmt::{self, Debug, Formatter};
use std::future::Future;
use std::iter;
use std::mem::{self, ManuallyDrop};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// A future that waits for multiple futures to complete.
///
/// This primitive is designed for testing scenarios where we want to wait for a batch
/// of futures to complete, and we want to control the polling behavior to avoid
/// busy-waiting or excessive polling.
///
/// Specifically, if N futures are pending, this future will wait for N wake signals
/// before waking the parent task. This assumes that each pending future will generate
/// at least one wake signal.
pub struct MultiAwait<F: Future> {
    /// We use `Option` here to allow us to drop the future once it has completed,
    /// while keeping the vector indices aligned with the `results` vector.
    /// This ensures that the output order matches the input order.
    futures: Vec<Option<Pin<Box<F>>>>,
    results: Vec<Option<F::Output>>,
    shared: Arc<SharedState>,
}

/// State shared between the `MultiAwait` future and the wakers passed to its child futures.
///
/// The `MultiAwait` future holds a reference to this state to check progress and reset counters.
/// Each child future is given a `Waker` that also holds a reference to this state (via a raw pointer
/// derived from the `Arc`). When a child future wakes its waker, it updates the counters in this state.
struct SharedState {
    parent_waker: Mutex<Option<Waker>>,
    wakes_needed: AtomicUsize,
    wakes_received: AtomicUsize,
}

impl<F: Future> Debug for MultiAwait<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let pending_count = self.futures.iter().filter(|f| f.is_some()).count();
        let ready_count = self.results.iter().filter(|r| r.is_some()).count();

        f.debug_struct(type_name::<Self>())
            .field("total_count", &self.futures.len())
            .field("pending_count", &pending_count)
            .field("ready_count", &ready_count)
            .finish_non_exhaustive()
    }
}

/// We implement `Unpin` manually because `F::Output` might be `!Unpin`, which would make
/// `MultiAwait` `!Unpin` by default (due to `results` vector). However, we never pin
/// the `results` or their contents, so it is safe to move `MultiAwait`.
impl<F: Future> Unpin for MultiAwait<F> {}

impl<F: Future> MultiAwait<F> {
    /// Creates a new `MultiAwait` future from an iterator of futures.
    pub fn new(futures: impl IntoIterator<Item = F>) -> Self {
        let futures: Vec<_> = futures.into_iter().map(|f| Some(Box::pin(f))).collect();
        let len = futures.len();
        Self {
            futures,
            results: iter::repeat_with(|| None).take(len).collect(),
            shared: Arc::new(SharedState {
                parent_waker: Mutex::new(None),
                wakes_needed: AtomicUsize::new(0),
                wakes_received: AtomicUsize::new(0),
            }),
        }
    }
}

impl<F: Future> Future for MultiAwait<F> {
    type Output = Vec<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();

        let pending_count = this.futures.iter().filter(|f| f.is_some()).count();

        if pending_count == 0 {
            return Poll::Ready(
                this.results
                    .iter_mut()
                    .map(|r| {
                        r.take().expect(
                            "results are initialized to the same length as futures, and we only \
                             complete the future when we set the result. If pending_count is 0, \
                             all futures have completed and thus all results must be present.",
                        )
                    })
                    .collect(),
            );
        }

        // Reset state for this poll cycle.
        // We expect 'pending_count' signals.
        this.shared.wakes_received.store(0, Ordering::SeqCst);
        this.shared
            .wakes_needed
            .store(pending_count, Ordering::SeqCst);

        // Update parent waker.
        *this.shared.parent_waker.lock() = Some(cx.waker().clone());

        // SAFETY: We are creating a Waker from a RawWaker that we constructed correctly
        // using the VTABLE and a pointer to our SharedState.
        let waker = unsafe { Waker::from_raw(raw_waker(&this.shared)) };
        let mut child_cx = Context::from_waker(&waker);

        for (i, future_slot) in this.futures.iter_mut().enumerate() {
            if let Some(future) = future_slot
                && let Poll::Ready(output) = future.as_mut().poll(&mut child_cx)
            {
                // We initialized results with the same length as futures,
                // and we are iterating with enumerate(), so i is always in bounds.
                #[allow(
                    clippy::indexing_slicing,
                    reason = "We initialized results with the same length as futures, and we are \
                              iterating with enumerate(), so i is always in bounds."
                )]
                {
                    this.results[i] = Some(output);
                }
                *future_slot = None;

                // If a future completes, we reduce the number of needed signals.
                // We don't need a signal from a completed future.
                //
                // We are just subtracting 1.
                #[allow(
                    clippy::arithmetic_side_effects,
                    reason = "We initialized wakes_needed to pending_count. Each future completes \
                              exactly once. Thus we subtract 1 at most pending_count times, so \
                              wakes_needed cannot go below 0."
                )]
                this.shared.wakes_needed.fetch_sub(1, Ordering::SeqCst);
            }
        }

        // Check if all are done now
        let pending_count_after = this.futures.iter().filter(|f| f.is_some()).count();
        if pending_count_after == 0 {
            return Poll::Ready(
                this.results
                    .iter_mut()
                    .map(|r| {
                        r.take().expect(
                            "results are initialized to the same length as futures, and we only \
                             complete the future when we set the result. If pending_count is 0, \
                             all futures have completed and thus all results must be present.",
                        )
                    })
                    .collect(),
            );
        }

        // If we have enough signals already (e.g. from concurrent completions or immediate wakes),
        // we should ensure we are woken up.
        let needed = this.shared.wakes_needed.load(Ordering::SeqCst);
        let received = this.shared.wakes_received.load(Ordering::SeqCst);

        if received >= needed {
            // We met the condition. Wake the parent to poll again immediately.
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}

fn raw_waker(shared: &Arc<SharedState>) -> RawWaker {
    let ptr = Arc::into_raw(Arc::clone(shared)) as *const ();
    RawWaker::new(ptr, &VTABLE)
}

const VTABLE: RawWakerVTable = RawWakerVTable::new(clone_waker, wake, wake_by_ref, drop_waker);

unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
    // SAFETY: The pointer was created by Arc::into_raw, so it is valid.
    // We are creating a new Arc instance to increment the ref count, then forgetting it
    // so the ref count stays incremented but we don't drop the Arc.
    let arc = unsafe { Arc::from_raw(ptr as *const SharedState) };
    mem::forget(Arc::clone(&arc)); // Increment ref count
    mem::forget(arc); // Don't drop the original
    RawWaker::new(ptr, &VTABLE)
}

unsafe fn wake(ptr: *const ()) {
    // SAFETY: The pointer was created by Arc::into_raw. We take ownership here
    // to consume the wake signal (drop the Arc).
    let arc = unsafe { Arc::from_raw(ptr as *const SharedState) };
    wake_impl(&arc);
    // arc is dropped here
}

unsafe fn wake_by_ref(ptr: *const ()) {
    // SAFETY: The pointer was created by Arc::into_raw. We wrap it in ManuallyDrop
    // because we are only borrowing it (wake_by_ref) and shouldn't drop the ref count.
    let arc = ManuallyDrop::new(unsafe { Arc::from_raw(ptr as *const SharedState) });
    wake_impl(&arc);
}

unsafe fn drop_waker(ptr: *const ()) {
    // SAFETY: The pointer was created by Arc::into_raw. We take ownership to drop it.
    drop(unsafe { Arc::from_raw(ptr as *const SharedState) });
}

fn wake_impl(shared: &SharedState) {
    // We are just adding 1. Overflow is extremely unlikely in this context.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Overflow is extremely unlikely in this context"
    )]
    let received = shared.wakes_received.fetch_add(1, Ordering::SeqCst) + 1;
    let needed = shared.wakes_needed.load(Ordering::SeqCst);

    if received >= needed
        && let Some(waker) = shared.parent_waker.lock().as_ref()
    {
        waker.wake_by_ref();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    struct ManualFuture {
        id: usize,
        state: Arc<ManualFutureState>,
    }

    struct ManualFutureState {
        waker: Mutex<Option<Waker>>,
        ready: AtomicBool,
    }

    impl ManualFuture {
        fn new(id: usize) -> Self {
            Self {
                id,
                state: Arc::new(ManualFutureState {
                    waker: Mutex::new(None),
                    ready: AtomicBool::new(false),
                }),
            }
        }
    }

    impl Future for ManualFuture {
        type Output = usize;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.state.ready.load(Ordering::SeqCst) {
                Poll::Ready(self.id)
            } else {
                *self.state.waker.lock() = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    struct FlagWaker;

    impl FlagWaker {
        #[allow(
            clippy::new_ret_no_self,
            reason = "This is a helper, not a constructor"
        )]
        fn new() -> (Arc<AtomicBool>, Waker) {
            let flag = Arc::new(AtomicBool::new(false));
            // SAFETY: We are creating a Waker from a RawWaker that we constructed correctly
            // using the VTABLE and a pointer to our AtomicBool.
            let waker = unsafe { Waker::from_raw(Self::raw_waker(Arc::clone(&flag))) };
            (flag, waker)
        }

        fn raw_waker(flag: Arc<AtomicBool>) -> RawWaker {
            let ptr = Arc::into_raw(flag) as *const ();
            RawWaker::new(
                ptr,
                &RawWakerVTable::new(
                    Self::clone_waker,
                    Self::wake,
                    Self::wake_by_ref,
                    Self::drop_waker,
                ),
            )
        }

        unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
            // SAFETY: The pointer was created by Arc::into_raw, so it is valid.
            let arc = unsafe { Arc::from_raw(ptr as *const AtomicBool) };
            mem::forget(Arc::clone(&arc));
            mem::forget(arc);
            RawWaker::new(
                ptr,
                &RawWakerVTable::new(
                    Self::clone_waker,
                    Self::wake,
                    Self::wake_by_ref,
                    Self::drop_waker,
                ),
            )
        }

        unsafe fn wake(ptr: *const ()) {
            // SAFETY: The pointer was created by Arc::into_raw. We take ownership here.
            let arc = unsafe { Arc::from_raw(ptr as *const AtomicBool) };
            arc.store(true, Ordering::SeqCst);
        }

        unsafe fn wake_by_ref(ptr: *const ()) {
            // SAFETY: The pointer was created by Arc::into_raw. We wrap it in ManuallyDrop.
            let arc = ManuallyDrop::new(unsafe { Arc::from_raw(ptr as *const AtomicBool) });
            arc.store(true, Ordering::SeqCst);
        }

        unsafe fn drop_waker(ptr: *const ()) {
            // SAFETY: The pointer was created by Arc::into_raw. We take ownership to drop it.
            drop(unsafe { Arc::from_raw(ptr as *const AtomicBool) });
        }
    }

    #[test]
    fn multi_await_waits_for_all_signals() {
        let _f1 = ManualFuture::new(1);
        let _f2 = ManualFuture::new(2);

        let s1 = Arc::new(ManualFutureState {
            waker: Mutex::new(None),
            ready: AtomicBool::new(false),
        });
        let s2 = Arc::new(ManualFutureState {
            waker: Mutex::new(None),
            ready: AtomicBool::new(false),
        });

        let f1 = ManualFuture {
            id: 1,
            state: Arc::clone(&s1),
        };
        let f2 = ManualFuture {
            id: 2,
            state: Arc::clone(&s2),
        };

        let mut multi = MultiAwait::new(vec![f1, f2]);
        let (woken, waker) = FlagWaker::new();
        let mut cx = Context::from_waker(&waker);

        // First poll
        assert!(matches!(Pin::new(&mut multi).poll(&mut cx), Poll::Pending));
        assert!(!woken.load(Ordering::SeqCst));

        // Signal first future
        s1.ready.store(true, Ordering::SeqCst);
        if let Some(w) = s1.waker.lock().take() {
            w.wake();
        }

        // Should NOT be woken yet (needs 2 signals)
        assert!(!woken.load(Ordering::SeqCst));

        // Signal second future
        s2.ready.store(true, Ordering::SeqCst);
        if let Some(w) = s2.waker.lock().take() {
            w.wake();
        }

        // Should be woken now
        assert!(woken.load(Ordering::SeqCst));

        // Second poll - should be ready
        match Pin::new(&mut multi).poll(&mut cx) {
            Poll::Ready(results) => assert_eq!(results, vec![1, 2]),
            Poll::Pending => panic!("Should be ready"),
        }
    }

    #[test]
    fn multi_await_partial_completion() {
        let s1 = Arc::new(ManualFutureState {
            waker: Mutex::new(None),
            ready: AtomicBool::new(true), // Already ready
        });
        let s2 = Arc::new(ManualFutureState {
            waker: Mutex::new(None),
            ready: AtomicBool::new(false),
        });

        let f1 = ManualFuture {
            id: 1,
            state: Arc::clone(&s1),
        };
        let f2 = ManualFuture {
            id: 2,
            state: Arc::clone(&s2),
        };

        let mut multi = MultiAwait::new(vec![f1, f2]);
        let (woken, waker) = FlagWaker::new();
        let mut cx = Context::from_waker(&waker);

        // First poll
        // f1 is ready, so it completes. needed becomes 1 (initially 2, -1 for completion).
        // f2 is pending.
        // received is 0.
        // received (0) < needed (1).
        // Returns Pending.
        assert!(matches!(Pin::new(&mut multi).poll(&mut cx), Poll::Pending));
        assert!(!woken.load(Ordering::SeqCst));

        // Signal f2
        s2.ready.store(true, Ordering::SeqCst);
        if let Some(w) = s2.waker.lock().take() {
            w.wake();
        }

        // Should be woken now (received 1 >= needed 1)
        assert!(woken.load(Ordering::SeqCst));

        // Second poll
        match Pin::new(&mut multi).poll(&mut cx) {
            Poll::Ready(results) => assert_eq!(results, vec![1, 2]),
            Poll::Pending => panic!("Should be ready"),
        }
    }
}
