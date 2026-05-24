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

    use events_once::{BoxedReceiver, BoxedSender, Event, EventPool, LocalEvent};
    use gungraun::prelude::*;

    type SyncEndpoints = (BoxedSender<i32>, Pin<Box<BoxedReceiver<i32>>>);

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

    fn make_sync_pool() -> EventPool<i32> {
        EventPool::<i32>::new()
    }

    // ---------- Full lifecycle ----------
    //
    // The full send-receive cycle, including event acquisition. The boxed case
    // pays one heap allocation; the pooled case does not (after pool warm-up,
    // which we perform in setup).

    #[library_benchmark]
    fn sync_boxed_lifecycle() {
        let (sender, receiver) = black_box(Event::<i32>::boxed());
        let mut receiver = Box::pin(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    #[library_benchmark]
    fn local_boxed_lifecycle() {
        let (sender, receiver) = black_box(LocalEvent::<i32>::boxed());
        let mut receiver = Box::pin(receiver);

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

    #[library_benchmark]
    #[bench::warm(make_warm_sync_pool())]
    fn sync_pooled_lifecycle(pool: EventPool<i32>) -> EventPool<i32> {
        let (sender, receiver) = black_box(pool.rent());
        let mut receiver = Box::pin(receiver);

        sender.send(black_box(42));

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
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

    library_benchmark_group!(
        name = lifecycle_group,
        benchmarks = [
            sync_boxed_lifecycle,
            local_boxed_lifecycle,
            sync_pooled_lifecycle,
        ]
    );

    library_benchmark_group!(
        name = partial_state_group,
        benchmarks = [
            sync_set_connected,
            sync_set_disconnected,
            sync_poll_connected,
            sync_poll_disconnected,
        ]
    );
}

#[cfg(target_os = "linux")]
pub use linux::{lifecycle_group, partial_state_group};

#[cfg(target_os = "linux")]
gungraun::main!(
    library_benchmark_groups = lifecycle_group,
    partial_state_group
);
