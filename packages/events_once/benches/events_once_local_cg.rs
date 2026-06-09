//! Callgrind benchmarks for the single-threaded (`LocalEvent`) facet of
//! the `events_once` crate.
//!
//! Paired with `events_once_local.rs` (wall-clock Criterion coverage).
//!
//! The scenarios cover the operations that make up the value proposition
//! of the crate: the full send-receive lifecycle (for boxed, pooled,
//! embedded and laked events), the partial-state hot paths that an event
//! exercises in real code (set / poll, against connected / disconnected
//! peers), and the two cancellation paths (sender dropped from BOUND /
//! AWAITING state).

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Tracking issue drafts live at \
          c:/Source/gungraun-lint-issues/ pending upstream filing."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::pin::Pin;
    use std::task::{self, Waker};

    use events_once::{
        BoxedLocalReceiver, BoxedLocalSender, EmbeddedLocalEvent, LocalEvent, LocalEventLake,
        LocalEventPool,
    };
    use gungraun::prelude::*;

    type LocalEndpoints = (BoxedLocalSender<i32>, Pin<Box<BoxedLocalReceiver<i32>>>);

    fn make_local_endpoints() -> LocalEndpoints {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        (sender, Box::pin(receiver))
    }

    fn make_local_sender_only() -> BoxedLocalSender<i32> {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        drop(receiver);
        sender
    }

    fn make_local_receiver_only() -> Pin<Box<BoxedLocalReceiver<i32>>> {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        drop(sender);
        Box::pin(receiver)
    }

    // The BOUND state is the initial state of every freshly constructed event: no value
    // has been set, no awaiter has been registered. Dropping the sender from this state
    // exercises the cheaper of the two cancellation paths because there is no waker to
    // consume and no wake to deliver - but it is also the less common case in practice,
    // see the AWAITING helper below.
    fn make_local_endpoints_bound() -> LocalEndpoints {
        make_local_endpoints()
    }

    // The AWAITING state is reached after the receiver has polled the event once and
    // registered a waker. In practice this is the most common state when a sender is
    // dropped without sending: most events that are ever awaited are awaited
    // immediately after subscription, so cancellation typically catches the event with
    // an awaiter already registered.
    fn make_local_endpoints_awaiting() -> LocalEndpoints {
        let (sender, mut receiver) = make_local_endpoints();
        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
        (sender, receiver)
    }

    fn make_local_pool() -> LocalEventPool<i32> {
        LocalEventPool::<i32>::new()
    }

    // Pool warm-up happens once during setup (renting one event then dropping
    // it back into the pool). The measured iteration is a steady-state
    // rent + send + poll, which avoids heap allocation.
    fn make_warm_local_pool() -> LocalEventPool<i32> {
        let pool = make_local_pool();
        let (sender, receiver) = pool.rent();
        drop(sender);
        drop(receiver);
        pool
    }

    // Lake lifecycle: like pooled, but routed through a type-erased lake that maintains
    // one underlying pool per TypeId. The warm path pre-creates the per-TypeId pool so
    // the steady-state iteration is rent + send + poll + release.
    fn make_warm_local_lake() -> LocalEventLake {
        let lake = LocalEventLake::new();
        let (sender, receiver) = lake.rent::<i32>();
        drop(sender);
        drop(receiver);
        lake
    }

    // Lifecycle benchmarks.
    //
    // The full send-receive cycle, including event acquisition. The boxed case
    // pays one heap allocation for the event itself; the pooled, embedded, and
    // laked cases do not (after pool warm-up, which we perform in setup where
    // applicable).
    //
    // Across all lifecycle benches the receiver is stack-pinned via
    // `std::pin::pin!` rather than `Box::pin` so that the measured iteration
    // reflects event mechanics rather than allocator overhead. See AGENTS.md
    // "Benchmark design" for the rationale (the cost of a `Box::pin` per
    // iteration is 40-50% of the measured count).

    #[library_benchmark]
    fn local_lifecycle_boxed() {
        let (sender, receiver) = black_box(LocalEvent::<i32>::boxed());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    #[library_benchmark]
    #[bench::warm(make_warm_local_pool())]
    fn local_lifecycle_pooled(pool: LocalEventPool<i32>) -> LocalEventPool<i32> {
        let (sender, receiver) = black_box(pool.rent());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    // The embedded lifecycle measures placing the event into caller-owned pinned storage,
    // sending a value, polling for it, and tearing down. The `place` itself must be
    // stack-pinned — that is the whole point of the embedded variant; boxing it would
    // measure boxed-event semantics instead. (Receiver stack-pinning follows the
    // group-wide convention documented above.)
    #[library_benchmark]
    fn local_lifecycle_embedded() {
        let mut place = std::pin::pin!(EmbeddedLocalEvent::<i32>::new());

        // SAFETY: `place` remains valid for writes and pinned for the entire body of this
        // function (it is a stack-pinned local that we never move). The endpoints we obtain
        // borrow `place` exclusively; we do not touch `place` again while they are alive and
        // they do not escape this function, so no conflicting reference to the event can
        // exist. `place` was just created and is not already in use by another event.
        let (sender, receiver) = black_box(unsafe { LocalEvent::placed(place.as_mut()) });
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    #[library_benchmark]
    #[bench::warm(make_warm_local_lake())]
    fn local_lifecycle_lake(lake: LocalEventLake) -> LocalEventLake {
        let (sender, receiver) = black_box(lake.rent::<i32>());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));

        lake
    }

    // Partial-state hot paths.
    //
    // These match the existing `events_once_local.rs` Criterion scenarios, isolating
    // the cost of a single send or poll without including the cost of the other
    // operation in the same iteration.

    #[library_benchmark]
    #[bench::connected(make_local_endpoints())]
    fn local_set_connected(input: LocalEndpoints) -> Pin<Box<BoxedLocalReceiver<i32>>> {
        let (sender, receiver) = input;
        sender.send(black_box(42));
        receiver
    }

    #[library_benchmark]
    #[bench::disconnected(make_local_sender_only())]
    fn local_set_disconnected(sender: BoxedLocalSender<i32>) {
        sender.send(black_box(42));
    }

    #[library_benchmark]
    #[bench::connected(make_local_endpoints())]
    fn local_poll_connected(input: LocalEndpoints) -> LocalEndpoints {
        let (sender, mut receiver) = input;
        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
        (sender, receiver)
    }

    #[library_benchmark]
    #[bench::disconnected(make_local_receiver_only())]
    fn local_poll_disconnected(
        mut receiver: Pin<Box<BoxedLocalReceiver<i32>>>,
    ) -> Pin<Box<BoxedLocalReceiver<i32>>> {
        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
        receiver
    }

    // Cancellation paths.
    //
    // Sender dropped without sending. Two sub-cases are measured because cancellation can
    // catch the event in different states with very different costs:
    //
    // - BOUND: the receiver has not yet polled. No waker is registered, no wake to deliver.
    // - AWAITING: the receiver has polled and parked a waker. We must wake the receiver
    //   before we publish the terminal DISCONNECTED state. AWAITING is the more common
    //   state in practice: most events that get awaited are awaited immediately after
    //   subscription.

    #[library_benchmark]
    #[bench::bound(make_local_endpoints_bound())]
    fn local_sender_dropped_from_bound(input: LocalEndpoints) -> Pin<Box<BoxedLocalReceiver<i32>>> {
        let (sender, receiver) = input;
        drop(sender);
        receiver
    }

    #[library_benchmark]
    #[bench::awaiting(make_local_endpoints_awaiting())]
    fn local_sender_dropped_from_awaiting(
        input: LocalEndpoints,
    ) -> Pin<Box<BoxedLocalReceiver<i32>>> {
        let (sender, receiver) = input;
        drop(sender);
        receiver
    }

    library_benchmark_group!(
        name = local,
        benchmarks = [
            local_lifecycle_boxed,
            local_lifecycle_pooled,
            local_lifecycle_embedded,
            local_lifecycle_lake,
            local_set_connected,
            local_set_disconnected,
            local_poll_connected,
            local_poll_disconnected,
            local_sender_dropped_from_bound,
            local_sender_dropped_from_awaiting,
        ]
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::local;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = local
);
