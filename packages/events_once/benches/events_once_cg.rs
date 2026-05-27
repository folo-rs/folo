//! Callgrind benchmarks for the `events_once` crate.
//!
//! Paired with `events_once_vs_3p.rs`, `events_once_sync.rs`, and `events_once_local.rs`
//! which cover the same operations under wall-clock measurement.
//!
//! The scenarios cover the four operations that make up the value proposition
//! of the crate: the full send-receive lifecycle (for boxed and pooled
//! events, sync and local), and the partial-state hot paths that an event
//! exercises in real code (set / poll, against connected / disconnected
//! peers).

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
        BoxedLocalReceiver, BoxedLocalSender, BoxedReceiver, BoxedSender, EmbeddedEvent,
        EmbeddedLocalEvent, Event, EventLake, EventPool, LocalEvent, LocalEventLake,
        LocalEventPool,
    };
    use gungraun::prelude::*;

    type SyncEndpoints = (BoxedSender<i32>, Pin<Box<BoxedReceiver<i32>>>);
    type LocalEndpoints = (BoxedLocalSender<i32>, Pin<Box<BoxedLocalReceiver<i32>>>);

    fn make_sync_endpoints() -> SyncEndpoints {
        let (sender, receiver) = Event::<i32>::boxed();
        (sender, Box::pin(receiver))
    }

    fn make_sync_sender_only() -> BoxedSender<i32> {
        let (sender, receiver) = Event::<i32>::boxed();
        drop(receiver);
        sender
    }

    fn make_sync_receiver_only() -> Pin<Box<BoxedReceiver<i32>>> {
        let (sender, receiver) = Event::<i32>::boxed();
        drop(sender);
        Box::pin(receiver)
    }

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

    fn make_sync_endpoints_bound() -> SyncEndpoints {
        // The BOUND state is the initial state of every freshly constructed event: no value
        // has been set, no awaiter has been registered. Dropping the sender from this state
        // is the fast path of cancellation - the receiver has not yet asked for the value.
        make_sync_endpoints()
    }

    fn make_local_endpoints_bound() -> LocalEndpoints {
        make_local_endpoints()
    }

    fn make_sync_pool() -> EventPool<i32> {
        EventPool::<i32>::new()
    }

    fn make_local_pool() -> LocalEventPool<i32> {
        LocalEventPool::<i32>::new()
    }

    // ---------- Full lifecycle ----------
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
    fn sync_boxed_lifecycle() {
        let (sender, receiver) = black_box(Event::<i32>::boxed());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    #[library_benchmark]
    fn local_boxed_lifecycle() {
        let (sender, receiver) = black_box(LocalEvent::<i32>::boxed());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    // Pool warm-up happens once during setup (renting one event then dropping
    // it back into the pool). The measured iteration is a steady-state
    // rent + send + poll, which avoids heap allocation.
    fn make_warm_sync_pool() -> EventPool<i32> {
        let pool = make_sync_pool();
        let (sender, receiver) = pool.rent();
        drop(sender);
        drop(receiver);
        pool
    }

    fn make_warm_local_pool() -> LocalEventPool<i32> {
        let pool = make_local_pool();
        let (sender, receiver) = pool.rent();
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::warm(make_warm_sync_pool())]
    fn sync_pooled_lifecycle(pool: EventPool<i32>) -> EventPool<i32> {
        let (sender, receiver) = black_box(pool.rent());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    #[library_benchmark]
    #[bench::warm(make_warm_local_pool())]
    fn local_pooled_lifecycle(pool: LocalEventPool<i32>) -> LocalEventPool<i32> {
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
    fn sync_embedded_lifecycle() {
        let mut place = std::pin::pin!(EmbeddedEvent::<i32>::new());

        // SAFETY: `place` remains valid for writes and pinned for the entire body of this
        // function (it is a stack-pinned local that we never move). The endpoints we obtain
        // borrow `place` exclusively; we do not touch `place` again while they are alive and
        // they do not escape this function, so no conflicting reference to the event can
        // exist. `place` was just created and is not already in use by another event.
        let (sender, receiver) = black_box(unsafe { Event::placed(place.as_mut()) });
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    #[library_benchmark]
    fn local_embedded_lifecycle() {
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

    // Lake lifecycle: like pooled, but routed through a type-erased lake that maintains
    // one underlying pool per TypeId. The warm path pre-creates the per-TypeId pool so
    // the steady-state iteration is rent + send + poll + release.
    fn make_warm_sync_lake() -> EventLake {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<i32>();
        drop(sender);
        drop(receiver);
        lake
    }

    fn make_warm_local_lake() -> LocalEventLake {
        let lake = LocalEventLake::new();
        let (sender, receiver) = lake.rent::<i32>();
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::warm(make_warm_sync_lake())]
    fn sync_lake_lifecycle(lake: EventLake) -> EventLake {
        let (sender, receiver) = black_box(lake.rent::<i32>());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));

        lake
    }

    #[library_benchmark]
    #[bench::warm(make_warm_local_lake())]
    fn local_lake_lifecycle(lake: LocalEventLake) -> LocalEventLake {
        let (sender, receiver) = black_box(lake.rent::<i32>());
        let mut receiver = std::pin::pin!(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));

        lake
    }

    // ---------- Partial-state hot paths ----------
    //
    // These match the existing `events_once_sync.rs` Criterion scenarios, isolating
    // the cost of a single send or poll without including the cost of the other
    // operation in the same iteration.

    #[library_benchmark]
    #[bench::connected(make_sync_endpoints())]
    fn sync_set_connected(input: SyncEndpoints) -> Pin<Box<BoxedReceiver<i32>>> {
        let (sender, receiver) = input;
        sender.send(black_box(42));
        receiver
    }

    #[library_benchmark]
    #[bench::disconnected(make_sync_sender_only())]
    fn sync_set_disconnected(sender: BoxedSender<i32>) {
        sender.send(black_box(42));
    }

    #[library_benchmark]
    #[bench::connected(make_sync_endpoints())]
    fn sync_poll_connected(input: SyncEndpoints) -> SyncEndpoints {
        let (sender, mut receiver) = input;
        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
        (sender, receiver)
    }

    #[library_benchmark]
    #[bench::disconnected(make_sync_receiver_only())]
    fn sync_poll_disconnected(
        mut receiver: Pin<Box<BoxedReceiver<i32>>>,
    ) -> Pin<Box<BoxedReceiver<i32>>> {
        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
        receiver
    }

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

    // ---------- Cancellation paths ----------
    //
    // Sender dropped without sending, from BOUND state - the receiver has not yet
    // polled. This is the fast cancellation path: no waker is registered, the
    // sender just signals DISCONNECTED.

    #[library_benchmark]
    #[bench::bound(make_sync_endpoints_bound())]
    fn sync_sender_dropped_from_bound(input: SyncEndpoints) -> Pin<Box<BoxedReceiver<i32>>> {
        let (sender, receiver) = input;
        drop(sender);
        receiver
    }

    #[library_benchmark]
    #[bench::bound(make_local_endpoints_bound())]
    fn local_sender_dropped_from_bound(input: LocalEndpoints) -> Pin<Box<BoxedLocalReceiver<i32>>> {
        let (sender, receiver) = input;
        drop(sender);
        receiver
    }

    library_benchmark_group!(
        name = lifecycle_group,
        benchmarks = [
            sync_boxed_lifecycle,
            local_boxed_lifecycle,
            sync_pooled_lifecycle,
            local_pooled_lifecycle,
            sync_embedded_lifecycle,
            local_embedded_lifecycle,
            sync_lake_lifecycle,
            local_lake_lifecycle,
        ]
    );

    library_benchmark_group!(
        name = partial_state_group,
        benchmarks = [
            sync_set_connected,
            sync_set_disconnected,
            local_set_connected,
            local_set_disconnected,
            sync_poll_connected,
            sync_poll_disconnected,
            local_poll_connected,
            local_poll_disconnected,
            sync_sender_dropped_from_bound,
            local_sender_dropped_from_bound,
        ]
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{lifecycle_group, partial_state_group};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = lifecycle_group, partial_state_group
);
