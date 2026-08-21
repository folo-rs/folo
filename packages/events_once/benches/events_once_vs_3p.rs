//! Third-party comparison benchmark: `events_once` against the `oneshot` crate.
//!
//! This file exists to answer one question - how does an `events_once` send/receive
//! round trip compare to the equivalent round trip on a well-known third-party oneshot
//! channel. Our storage strategies are registered as leaves of the same group as the
//! `oneshot` leaf, so Criterion reports our numbers and the competitor's number side by
//! side in one table. The compared strategies are boxed, pooled, raw-pooled, lake and
//! raw-lake, each in its single-threaded and thread-safe form.
//!
//! Embedded events are deliberately absent. Their acquisition places the event into
//! storage the caller already owns, so a fair measurement has to allocate that storage
//! per iteration outside the timed region and hand it back afterwards - an
//! `iter_batched` harness rather than the `iter` harness the rest of this file uses. The
//! competitor has no equivalent to compare against, so the row would sit in this table
//! measured differently from every other row and matched against nothing. It is covered
//! by `events_once_ops.rs`, where a batched harness is the norm.
//!
//! Two groups cover the two shapes a oneshot round trip takes in practice:
//!
//! * `single_poll` - the value is sent before the receiver is ever polled, so delivery
//!   never touches the awaiting state.
//! * `two_poll` - the receiver polls first (parking a waker) and the value arrives
//!   afterwards, so delivery runs through the awaiting state and wakes the receiver.
//!
//! The identifiers of the groups and their leaves are load-bearing: they are the join
//! key of this benchmark's measurement history, so they are preserved verbatim even
//! where the internal benchmark suite (`events_once_ops.rs`) names the same concepts
//! differently. That suite is the canonical scenario matrix for our own operations;
//! this file is the dedicated competitor comparison and nothing else.
//!
//! Everything that is not the round trip is prepared outside the measured region: pools
//! and lakes are created and warmed by the shared `warm_*` functions before the group
//! runs, and the noop-waker polling context is built before `iter`. Both groups warm
//! through those same functions, so no container can end up in a different state in one
//! group than in the other. Receivers are stack-pinned so that no iteration pays for a
//! benchmark-owned heap allocation that a user would not pay for. Correctness is
//! the test suite's job; measured closures consume their results through `black_box`
//! and never assert or unwrap, so no leaf pays for a validation branch - which matters
//! doubly here, where a fallible third-party API is compared against our infallible one.

#![expect(clippy::undocumented_unsafe_blocks, reason = "benchmarks")]

use std::hint::black_box;
use std::pin::{Pin, pin};
use std::task::{self, Waker};

use criterion::{Criterion, criterion_group, criterion_main};
use events_once::{
    Event, EventLake, EventPool, LocalEvent, LocalEventLake, LocalEventPool, RawEventLake,
    RawEventPool, RawLocalEventLake, RawLocalEventPool,
};

/// Arbitrary payload. The event machinery treats the payload as opaque, so the specific
/// value only needs to be stable across scenarios to keep them comparable.
const PAYLOAD: i32 = 42;

/// A polling context built on the static noop waker. It carries no per-event state, so
/// one prepared instance serves every poll of a benchmark.
type NoopContext = task::Context<'static>;

/// A raw pool must stay pinned at one address for as long as endpoints rented from it
/// exist. Heap pinning is what lets it cross the function boundary out of its warm-up
/// function; the pin happens once per group, outside every measured region.
type LocalRawPool = Pin<Box<RawLocalEventPool<i32>>>;
type SyncRawPool = Pin<Box<RawEventPool<i32>>>;

fn noop_context() -> NoopContext {
    task::Context::from_waker(Waker::noop())
}

// Storage warm-up, one function per container: rent one event and return it, so a
// measured region that rents works against a container that already owns a recycled
// event and does not allocate. Both groups build their containers exclusively through
// these functions, which is what makes the warm state identical across groups by
// construction. Measuring a first-touch allocation against a competitor that is warm by
// construction would compare container growth, not the round trip.

fn warm_local_pool() -> LocalEventPool<i32> {
    let pool = LocalEventPool::<i32>::new();
    drop(pool.rent());
    pool
}

fn warm_sync_pool() -> EventPool<i32> {
    let pool = EventPool::<i32>::new();
    drop(pool.rent());
    pool
}

fn warm_local_raw_pool() -> LocalRawPool {
    let pool = Box::pin(RawLocalEventPool::<i32>::new());

    // SAFETY: the endpoints are dropped before this function returns, so they do not
    // outlive the pool.
    drop(unsafe { pool.as_ref().rent() });

    pool
}

fn warm_sync_raw_pool() -> SyncRawPool {
    let pool = Box::pin(RawEventPool::<i32>::new());

    // SAFETY: the endpoints are dropped before this function returns, so they do not
    // outlive the pool.
    drop(unsafe { pool.as_ref().rent() });

    pool
}

fn warm_local_lake() -> LocalEventLake {
    let lake = LocalEventLake::new();
    drop(lake.rent::<i32>());
    lake
}

fn warm_sync_lake() -> EventLake {
    let lake = EventLake::new();
    drop(lake.rent::<i32>());
    lake
}

fn warm_local_raw_lake() -> RawLocalEventLake {
    let lake = RawLocalEventLake::new();

    // SAFETY: the endpoints are dropped before this function returns, so they do not
    // outlive the lake.
    drop(unsafe { lake.rent::<i32>() });

    lake
}

fn warm_sync_raw_lake() -> RawEventLake {
    let lake = RawEventLake::new();

    // SAFETY: the endpoints are dropped before this function returns, so they do not
    // outlive the lake.
    drop(unsafe { lake.rent::<i32>() });

    lake
}

// Registers a send-first round trip: acquire the endpoints, send, poll out the value and
// release the storage - all inside the measured region. The polling context is built
// before `iter`, so no iteration pays for it.
macro_rules! bench_send_receive {
    ($group:expr, $id:literal, $acquire:expr) => {
        $group.bench_function($id, |b| {
            let mut cx = noop_context();

            b.iter(|| {
                let (sender, receiver) = black_box($acquire);
                let mut receiver = pin!(receiver);

                sender.send(black_box(PAYLOAD));

                _ = black_box(receiver.as_mut().poll(&mut cx));
            });
        });
    };
}

// Registers an await-first round trip: the receiver polls (and parks a waker) before the
// value is sent, so delivery runs through the awaiting state and wakes the receiver.
// Acquisition and release are inside the measured region, as above.
macro_rules! bench_send_receive_2poll {
    ($group:expr, $id:literal, $acquire:expr) => {
        $group.bench_function($id, |b| {
            let mut cx = noop_context();

            b.iter(|| {
                let (sender, receiver) = black_box($acquire);
                let mut receiver = pin!(receiver);

                _ = black_box(receiver.as_mut().poll(&mut cx));

                sender.send(black_box(PAYLOAD));

                _ = black_box(receiver.as_mut().poll(&mut cx));
            });
        });
    };
}

fn single_poll(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_vs_3p/single_poll");

    let local_pool = warm_local_pool();
    let sync_pool = warm_sync_pool();
    let local_raw_pool = warm_local_raw_pool();
    let sync_raw_pool = warm_sync_raw_pool();
    let local_lake = warm_local_lake();
    let sync_lake = warm_sync_lake();
    let local_raw_lake = warm_local_raw_lake();
    let sync_raw_lake = warm_sync_raw_lake();

    bench_send_receive!(g, "local_boxed_send_receive", LocalEvent::<i32>::boxed());
    bench_send_receive!(g, "sync_boxed_send_receive", Event::<i32>::boxed());

    bench_send_receive!(g, "local_pooled_send_receive", local_pool.rent());
    bench_send_receive!(g, "sync_pooled_send_receive", sync_pool.rent());

    // SAFETY (raw pool and lake rows): the pool or lake outlives every rented endpoint
    // because it is created before the benchmark group and the endpoints never escape
    // the closure that rents them.
    bench_send_receive!(g, "local_raw_pooled_send_receive", unsafe {
        local_raw_pool.as_ref().rent()
    });
    bench_send_receive!(g, "sync_raw_pooled_send_receive", unsafe {
        sync_raw_pool.as_ref().rent()
    });

    bench_send_receive!(g, "local_lake_send_receive", local_lake.rent::<i32>());
    bench_send_receive!(g, "sync_lake_send_receive", sync_lake.rent::<i32>());

    bench_send_receive!(g, "local_raw_lake_send_receive", unsafe {
        local_raw_lake.rent::<i32>()
    });
    bench_send_receive!(g, "sync_raw_lake_send_receive", unsafe {
        sync_raw_lake.rent::<i32>()
    });

    // The competitor. `oneshot` reports send failures, so the result is consumed through
    // `black_box` instead of being unwrapped: an unwrap would add a branch that the
    // infallible `events_once` senders above do not have, which would tilt the very
    // comparison this file exists to make.
    g.bench_function("oneshot_send_receive", |b| {
        let mut cx = noop_context();

        b.iter(|| {
            let (sender, receiver) = black_box(oneshot::channel::<i32>());
            let mut receiver = pin!(receiver.into_future());

            _ = black_box(sender.send(black_box(PAYLOAD)));

            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.finish();
}

fn two_poll(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_vs_3p/two_poll");

    let local_pool = warm_local_pool();
    let sync_pool = warm_sync_pool();
    let local_raw_pool = warm_local_raw_pool();
    let sync_raw_pool = warm_sync_raw_pool();
    let local_lake = warm_local_lake();
    let sync_lake = warm_sync_lake();
    let local_raw_lake = warm_local_raw_lake();
    let sync_raw_lake = warm_sync_raw_lake();

    bench_send_receive_2poll!(
        g,
        "local_boxed_send_receive_2poll",
        LocalEvent::<i32>::boxed()
    );
    bench_send_receive_2poll!(g, "sync_boxed_send_receive_2poll", Event::<i32>::boxed());

    bench_send_receive_2poll!(g, "local_pooled_send_receive_2poll", local_pool.rent());
    bench_send_receive_2poll!(g, "sync_pooled_send_receive_2poll", sync_pool.rent());

    // SAFETY (raw pool and lake rows): the pool or lake outlives every rented endpoint
    // because it is created before the benchmark group and the endpoints never escape
    // the closure that rents them.
    bench_send_receive_2poll!(g, "local_raw_pooled_send_receive_2poll", unsafe {
        local_raw_pool.as_ref().rent()
    });
    bench_send_receive_2poll!(g, "sync_raw_pooled_send_receive_2poll", unsafe {
        sync_raw_pool.as_ref().rent()
    });

    bench_send_receive_2poll!(g, "local_lake_send_receive_2poll", local_lake.rent::<i32>());
    bench_send_receive_2poll!(g, "sync_lake_send_receive_2poll", sync_lake.rent::<i32>());

    bench_send_receive_2poll!(g, "local_raw_lake_send_receive_2poll", unsafe {
        local_raw_lake.rent::<i32>()
    });
    bench_send_receive_2poll!(g, "sync_raw_lake_send_receive_2poll", unsafe {
        sync_raw_lake.rent::<i32>()
    });

    // The competitor, polled before and after the send, as above.
    g.bench_function("oneshot_send_receive_2poll", |b| {
        let mut cx = noop_context();

        b.iter(|| {
            let (sender, receiver) = black_box(oneshot::channel::<i32>());
            let mut receiver = pin!(receiver.into_future());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            _ = black_box(sender.send(black_box(PAYLOAD)));

            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.finish();
}

criterion_group!(benches, single_poll, two_poll);
criterion_main!(benches);
