//! Wall-clock (Criterion) half of the canonical benchmark scenario matrix of the
//! `events_once` package.
//!
//! Paired with `events_once_ops_cg.rs`, which measures the same scenarios under
//! Callgrind. The scenario matrix itself - which fixes, for every row, the threading
//! model, storage strategy, start state, timed operation and cleanup boundary - is
//! documented in `packages/events_once/AGENTS.md` ("Canonical benchmark scenario
//! matrix"). Both files follow that matrix, and a Callgrind row exists only where the
//! identically prepared Criterion row exists here.
//!
//! Everything that is not the operation under test is prepared outside the measured
//! region: pools and lakes are created and warmed before the group runs, endpoints and
//! caller-owned embedded storage come from the `iter_batched` preparation callback,
//! and the noop-waker polling context is built before `iter`. The Callgrind twin
//! prepares the same values with functions of the same names, which Gungraun evaluates
//! outside its own measured region.
//!
//! Equivalent `LocalEvent` and `Event` scenarios are registered as leaves of the same
//! group so that Criterion reports them side by side: the package requires the
//! single-threaded variant to stay at least as fast as the thread-safe one. The
//! lifecycle groups additionally carry a third-party `oneshot` leaf, which is the
//! external reference point for the same lifecycle.
//!
//! Correctness is the test suite's job. Measured closures consume their results
//! through `black_box` and never assert, so that no benchmark pays for validation
//! branches that the measured API does not itself perform.

#![expect(clippy::undocumented_unsafe_blocks, reason = "benchmarks")]

use std::hint::black_box;
use std::pin::{Pin, pin};
use std::task::{self, Waker};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use events_once::{
    BoxedLocalReceiver, BoxedLocalSender, BoxedReceiver, BoxedSender, EmbeddedEvent,
    EmbeddedLocalEvent, Event, EventLake, EventPool, LocalEvent, LocalEventLake, LocalEventPool,
    PooledLocalReceiver, PooledLocalSender, PooledReceiver, PooledSender, RawEventLake,
    RawEventPool, RawLocalEventLake, RawLocalEventPool, RawLocalPooledReceiver,
    RawLocalPooledSender, RawLocalReceiver, RawLocalSender, RawPooledReceiver, RawPooledSender,
    RawReceiver, RawSender,
};

/// Arbitrary payload. The event machinery treats the payload as opaque, so the
/// specific value only needs to be stable across scenarios to keep them comparable.
const PAYLOAD: i32 = 42;

/// A polling context built on the static noop waker. It carries no per-event state,
/// so one prepared instance serves every poll of a benchmark.
type NoopContext = task::Context<'static>;

type LocalBoxedEndpoints = (BoxedLocalSender<i32>, BoxedLocalReceiver<i32>);
type SyncBoxedEndpoints = (BoxedSender<i32>, BoxedReceiver<i32>);
type LocalEmbeddedEndpoints = (RawLocalSender<i32>, RawLocalReceiver<i32>);
type SyncEmbeddedEndpoints = (RawSender<i32>, RawReceiver<i32>);
type LocalPooledEndpoints = (PooledLocalSender<i32>, PooledLocalReceiver<i32>);
type SyncPooledEndpoints = (PooledSender<i32>, PooledReceiver<i32>);
type LocalRawPooledEndpoints = (RawLocalPooledSender<i32>, RawLocalPooledReceiver<i32>);
type SyncRawPooledEndpoints = (RawPooledSender<i32>, RawPooledReceiver<i32>);

/// Caller-owned storage. The event is placed into it and released from it, but the
/// storage itself belongs to the benchmark, not to the event, which is what makes it
/// preparable outside the measured region. Heap pinning is what lets it cross a
/// function boundary; it is not the boxed-event strategy, where the event owns its own
/// allocation and frees it on release.
type LocalEmbeddedStorage = Pin<Box<EmbeddedLocalEvent<i32>>>;
type SyncEmbeddedStorage = Pin<Box<EmbeddedEvent<i32>>>;
type LocalRawPool = Pin<Box<RawLocalEventPool<i32>>>;
type SyncRawPool = Pin<Box<RawEventPool<i32>>>;

fn noop_context() -> NoopContext {
    task::Context::from_waker(Waker::noop())
}

// Brings a receiver to the awaiting state by polling it once, parking a waker.
// Receivers are `Unpin`, so this needs no boxing: the caller keeps ownership of an
// unboxed receiver that later measured regions can drop without freeing a
// benchmark-owned allocation.
fn park_waker<R: Future + Unpin>(receiver: &mut R) {
    let mut cx = noop_context();
    _ = black_box(Pin::new(receiver).poll(&mut cx));
}

// Storage warm-up: rent one event and return it, so a measured region that rents works
// against a pool that already owns a recycled event and does not allocate.

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

fn local_embedded_storage() -> LocalEmbeddedStorage {
    Box::pin(EmbeddedLocalEvent::<i32>::new())
}

fn sync_embedded_storage() -> SyncEmbeddedStorage {
    Box::pin(EmbeddedEvent::<i32>::new())
}

// Endpoint preparation, one pair of functions per storage strategy: a `bound` start
// state (nothing has happened yet) and an `awaiting` start state (the receiver has
// parked a waker). Whatever owns the storage travels with the endpoints so that the
// measured region can hand it back, keeping storage teardown untimed.

fn local_boxed_bound() -> LocalBoxedEndpoints {
    LocalEvent::<i32>::boxed()
}

fn sync_boxed_bound() -> SyncBoxedEndpoints {
    Event::<i32>::boxed()
}

fn local_boxed_awaiting() -> LocalBoxedEndpoints {
    let (sender, mut receiver) = local_boxed_bound();
    park_waker(&mut receiver);
    (sender, receiver)
}

fn sync_boxed_awaiting() -> SyncBoxedEndpoints {
    let (sender, mut receiver) = sync_boxed_bound();
    park_waker(&mut receiver);
    (sender, receiver)
}

fn local_embedded_bound() -> (LocalEmbeddedStorage, LocalEmbeddedEndpoints) {
    let mut place = local_embedded_storage();

    // SAFETY: the storage is heap-pinned and is handed to the caller together with the
    // endpoints, and every benchmark that receives them drops both endpoints before
    // returning the storage, so the storage stays valid and pinned for their whole
    // lifetime. The place was just created and is not in use by another event.
    let endpoints = unsafe { LocalEvent::placed(place.as_mut()) };

    (place, endpoints)
}

fn sync_embedded_bound() -> (SyncEmbeddedStorage, SyncEmbeddedEndpoints) {
    let mut place = sync_embedded_storage();

    // SAFETY: the storage is heap-pinned and is handed to the caller together with the
    // endpoints, and every benchmark that receives them drops both endpoints before
    // returning the storage, so the storage stays valid and pinned for their whole
    // lifetime. The place was just created and is not in use by another event.
    let endpoints = unsafe { Event::placed(place.as_mut()) };

    (place, endpoints)
}

fn local_embedded_awaiting() -> (LocalEmbeddedStorage, LocalEmbeddedEndpoints) {
    let (place, (sender, mut receiver)) = local_embedded_bound();
    park_waker(&mut receiver);
    (place, (sender, receiver))
}

fn sync_embedded_awaiting() -> (SyncEmbeddedStorage, SyncEmbeddedEndpoints) {
    let (place, (sender, mut receiver)) = sync_embedded_bound();
    park_waker(&mut receiver);
    (place, (sender, receiver))
}

fn local_pooled_bound() -> (LocalEventPool<i32>, LocalPooledEndpoints) {
    let pool = warm_local_pool();
    let endpoints = pool.rent();
    (pool, endpoints)
}

fn sync_pooled_bound() -> (EventPool<i32>, SyncPooledEndpoints) {
    let pool = warm_sync_pool();
    let endpoints = pool.rent();
    (pool, endpoints)
}

fn local_pooled_awaiting() -> (LocalEventPool<i32>, LocalPooledEndpoints) {
    let (pool, (sender, mut receiver)) = local_pooled_bound();
    park_waker(&mut receiver);
    (pool, (sender, receiver))
}

fn sync_pooled_awaiting() -> (EventPool<i32>, SyncPooledEndpoints) {
    let (pool, (sender, mut receiver)) = sync_pooled_bound();
    park_waker(&mut receiver);
    (pool, (sender, receiver))
}

fn local_raw_pooled_bound() -> (LocalRawPool, LocalRawPooledEndpoints) {
    let pool = warm_local_raw_pool();

    // SAFETY: the pool is heap-pinned and is handed to the caller together with the
    // endpoints, and every benchmark that receives them drops both endpoints before
    // returning the pool, so the pool outlives them.
    let endpoints = unsafe { pool.as_ref().rent() };

    (pool, endpoints)
}

fn sync_raw_pooled_bound() -> (SyncRawPool, SyncRawPooledEndpoints) {
    let pool = warm_sync_raw_pool();

    // SAFETY: the pool is heap-pinned and is handed to the caller together with the
    // endpoints, and every benchmark that receives them drops both endpoints before
    // returning the pool, so the pool outlives them.
    let endpoints = unsafe { pool.as_ref().rent() };

    (pool, endpoints)
}

fn local_raw_pooled_awaiting() -> (LocalRawPool, LocalRawPooledEndpoints) {
    let (pool, (sender, mut receiver)) = local_raw_pooled_bound();
    park_waker(&mut receiver);
    (pool, (sender, receiver))
}

fn sync_raw_pooled_awaiting() -> (SyncRawPool, SyncRawPooledEndpoints) {
    let (pool, (sender, mut receiver)) = sync_raw_pooled_bound();
    park_waker(&mut receiver);
    (pool, (sender, receiver))
}

fn local_lake_bound() -> (LocalEventLake, LocalPooledEndpoints) {
    let lake = warm_local_lake();
    let endpoints = lake.rent::<i32>();
    (lake, endpoints)
}

fn sync_lake_bound() -> (EventLake, SyncPooledEndpoints) {
    let lake = warm_sync_lake();
    let endpoints = lake.rent::<i32>();
    (lake, endpoints)
}

fn local_lake_awaiting() -> (LocalEventLake, LocalPooledEndpoints) {
    let (lake, (sender, mut receiver)) = local_lake_bound();
    park_waker(&mut receiver);
    (lake, (sender, receiver))
}

fn sync_lake_awaiting() -> (EventLake, SyncPooledEndpoints) {
    let (lake, (sender, mut receiver)) = sync_lake_bound();
    park_waker(&mut receiver);
    (lake, (sender, receiver))
}

fn local_raw_lake_bound() -> (RawLocalEventLake, LocalRawPooledEndpoints) {
    let lake = warm_local_raw_lake();

    // SAFETY: the lake is handed to the caller together with the endpoints, and every
    // benchmark that receives them drops both endpoints before returning the lake, so
    // the lake outlives them.
    let endpoints = unsafe { lake.rent::<i32>() };

    (lake, endpoints)
}

fn sync_raw_lake_bound() -> (RawEventLake, SyncRawPooledEndpoints) {
    let lake = warm_sync_raw_lake();

    // SAFETY: the lake is handed to the caller together with the endpoints, and every
    // benchmark that receives them drops both endpoints before returning the lake, so
    // the lake outlives them.
    let endpoints = unsafe { lake.rent::<i32>() };

    (lake, endpoints)
}

fn local_raw_lake_awaiting() -> (RawLocalEventLake, LocalRawPooledEndpoints) {
    let (lake, (sender, mut receiver)) = local_raw_lake_bound();
    park_waker(&mut receiver);
    (lake, (sender, receiver))
}

fn sync_raw_lake_awaiting() -> (RawEventLake, SyncRawPooledEndpoints) {
    let (lake, (sender, mut receiver)) = sync_raw_lake_bound();
    park_waker(&mut receiver);
    (lake, (sender, receiver))
}

// Inputs for the focused operations, which start from a state that a peer endpoint has
// already left behind.

fn local_boxed_sender_only() -> BoxedLocalSender<i32> {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    drop(receiver);
    sender
}

fn sync_boxed_sender_only() -> BoxedSender<i32> {
    let (sender, receiver) = Event::<i32>::boxed();
    drop(receiver);
    sender
}

fn local_boxed_set() -> BoxedLocalReceiver<i32> {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    sender.send(PAYLOAD);
    receiver
}

fn sync_boxed_set() -> BoxedReceiver<i32> {
    let (sender, receiver) = Event::<i32>::boxed();
    sender.send(PAYLOAD);
    receiver
}

fn local_boxed_disconnected() -> BoxedLocalReceiver<i32> {
    let (sender, receiver) = LocalEvent::<i32>::boxed();
    drop(sender);
    receiver
}

fn sync_boxed_disconnected() -> BoxedReceiver<i32> {
    let (sender, receiver) = Event::<i32>::boxed();
    drop(sender);
    receiver
}

fn rent(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/rent");
    let local_pool = warm_local_pool();
    let sync_pool = warm_sync_pool();
    let local_lake = warm_local_lake();
    let sync_lake = warm_sync_lake();

    g.bench_function("local/pooled", |b| {
        b.iter_batched(
            || local_pool.clone(),
            |pool| {
                let endpoints = black_box(pool.rent());
                black_box((pool, endpoints))
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/pooled", |b| {
        b.iter_batched(
            || sync_pool.clone(),
            |pool| {
                let endpoints = black_box(pool.rent());
                black_box((pool, endpoints))
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("local/lake", |b| {
        b.iter_batched(
            || local_lake.clone(),
            |lake| {
                let endpoints = black_box(lake.rent::<i32>());
                black_box((lake, endpoints))
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/lake", |b| {
        b.iter_batched(
            || sync_lake.clone(),
            |lake| {
                let endpoints = black_box(lake.rent::<i32>());
                black_box((lake, endpoints))
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// Registers a send-first lifecycle: acquire the endpoints, send, poll out the value and
// release the storage - all inside the measured region. The polling context is built
// before `iter`, so no iteration pays for it.
macro_rules! bench_lifecycle_send_first {
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

// Registers an await-first lifecycle: the receiver polls (and parks a waker) before the
// value is sent, so delivery runs through the awaiting state and wakes the receiver.
// Acquisition and release are inside the measured region, as above.
macro_rules! bench_lifecycle_await_first {
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

// Registers a cancellation row for a storage strategy that hands its owner along with
// the endpoints. The owner is returned from the measured closure so that its teardown
// stays outside the measured region. The trailing token selects which endpoint is
// dropped first; the endpoint pair is always destructured in its true order, because a
// tuple pattern binds by position and not by the name written in the pattern.
macro_rules! bench_cancel_owned {
    ($group:expr, $id:literal, $setup:expr, sender_first) => {
        $group.bench_function($id, |b| {
            b.iter_batched(
                $setup,
                |(owner, (sender, receiver))| {
                    drop(sender);
                    drop(receiver);
                    owner
                },
                BatchSize::SmallInput,
            );
        });
    };
    ($group:expr, $id:literal, $setup:expr, receiver_first) => {
        $group.bench_function($id, |b| {
            b.iter_batched(
                $setup,
                |(owner, (sender, receiver))| {
                    drop(receiver);
                    drop(sender);
                    owner
                },
                BatchSize::SmallInput,
            );
        });
    };
}

// Registers a cancellation row for boxed events, which own their storage and therefore
// have no owner to hand back.
macro_rules! bench_cancel_boxed {
    ($group:expr, $id:literal, $setup:expr, sender_first) => {
        $group.bench_function($id, |b| {
            b.iter_batched(
                $setup,
                |(sender, receiver)| {
                    drop(sender);
                    drop(receiver);
                },
                BatchSize::SmallInput,
            );
        });
    };
    ($group:expr, $id:literal, $setup:expr, receiver_first) => {
        $group.bench_function($id, |b| {
            b.iter_batched(
                $setup,
                |(sender, receiver)| {
                    drop(receiver);
                    drop(sender);
                },
                BatchSize::SmallInput,
            );
        });
    };
}

fn lifecycle(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/lifecycle");

    // Every pool and lake is warmed before the group runs, so the first measured
    // iteration already rents recycled storage instead of allocating - the same state
    // the Callgrind twins are handed by their setup functions. The raw pools are warmed
    // in place because they must stay pinned at one address.
    let local_pool = warm_local_pool();
    let sync_pool = warm_sync_pool();
    let local_raw_pool = warm_local_raw_pool();
    let sync_raw_pool = warm_sync_raw_pool();
    let local_lake = warm_local_lake();
    let sync_lake = warm_sync_lake();
    let local_raw_lake = warm_local_raw_lake();
    let sync_raw_lake = warm_sync_raw_lake();

    bench_lifecycle_send_first!(g, "local/boxed", LocalEvent::<i32>::boxed());
    bench_lifecycle_send_first!(g, "sync/boxed", Event::<i32>::boxed());

    // The embedded rows acquire by placing the event into caller-owned storage that the
    // preparation callback allocated, so the measured region contains placement and
    // release but no storage allocation. That is the difference between embedding an
    // event in an object the caller already owns and letting the event own its own
    // allocation, which is what the boxed rows above measure.
    g.bench_function("local/embedded", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            local_embedded_storage,
            |mut place| {
                {
                    // SAFETY: `place` is heap-pinned, outlives this scope, and is not
                    // touched while the endpoints borrow it. The endpoints do not escape
                    // the scope, so they are gone before the storage is returned. The
                    // place was freshly created and is not in use by another event.
                    let (sender, receiver) =
                        black_box(unsafe { LocalEvent::placed(place.as_mut()) });
                    let mut receiver = pin!(receiver);

                    sender.send(black_box(PAYLOAD));

                    _ = black_box(receiver.as_mut().poll(&mut cx));
                }

                place
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/embedded", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            sync_embedded_storage,
            |mut place| {
                {
                    // SAFETY: `place` is heap-pinned, outlives this scope, and is not
                    // touched while the endpoints borrow it. The endpoints do not escape
                    // the scope, so they are gone before the storage is returned. The
                    // place was freshly created and is not in use by another event.
                    let (sender, receiver) = black_box(unsafe { Event::placed(place.as_mut()) });
                    let mut receiver = pin!(receiver);

                    sender.send(black_box(PAYLOAD));

                    _ = black_box(receiver.as_mut().poll(&mut cx));
                }

                place
            },
            BatchSize::SmallInput,
        );
    });

    bench_lifecycle_send_first!(g, "local/pooled", local_pool.rent());
    bench_lifecycle_send_first!(g, "sync/pooled", sync_pool.rent());

    // SAFETY (raw pool and lake rows): the pool or lake outlives every rented endpoint
    // because it is created before the benchmark group and the endpoints never escape
    // the closure that rents them.
    bench_lifecycle_send_first!(g, "local/raw_pooled", unsafe {
        local_raw_pool.as_ref().rent()
    });
    bench_lifecycle_send_first!(g, "sync/raw_pooled", unsafe {
        sync_raw_pool.as_ref().rent()
    });

    bench_lifecycle_send_first!(g, "local/lake", local_lake.rent::<i32>());
    bench_lifecycle_send_first!(g, "sync/lake", sync_lake.rent::<i32>());

    bench_lifecycle_send_first!(g, "local/raw_lake", unsafe { local_raw_lake.rent::<i32>() });
    bench_lifecycle_send_first!(g, "sync/raw_lake", unsafe { sync_raw_lake.rent::<i32>() });

    // External reference point. `oneshot` reports send failures, so the result is
    // consumed through `black_box` instead of being unwrapped: an unwrap would add a
    // branch that the infallible `events_once` senders do not have.
    g.bench_function("oneshot", |b| {
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

fn lifecycle_await_first(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/lifecycle_await_first");

    let local_pool = warm_local_pool();
    let sync_pool = warm_sync_pool();
    let local_raw_pool = warm_local_raw_pool();
    let sync_raw_pool = warm_sync_raw_pool();
    let local_lake = warm_local_lake();
    let sync_lake = warm_sync_lake();
    let local_raw_lake = warm_local_raw_lake();
    let sync_raw_lake = warm_sync_raw_lake();

    bench_lifecycle_await_first!(g, "local/boxed", LocalEvent::<i32>::boxed());
    bench_lifecycle_await_first!(g, "sync/boxed", Event::<i32>::boxed());

    g.bench_function("local/embedded", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            local_embedded_storage,
            |mut place| {
                {
                    // SAFETY: `place` is heap-pinned, outlives this scope, and is not
                    // touched while the endpoints borrow it. The endpoints do not escape
                    // the scope, so they are gone before the storage is returned. The
                    // place was freshly created and is not in use by another event.
                    let (sender, receiver) =
                        black_box(unsafe { LocalEvent::placed(place.as_mut()) });
                    let mut receiver = pin!(receiver);

                    _ = black_box(receiver.as_mut().poll(&mut cx));

                    sender.send(black_box(PAYLOAD));

                    _ = black_box(receiver.as_mut().poll(&mut cx));
                }

                place
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/embedded", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            sync_embedded_storage,
            |mut place| {
                {
                    // SAFETY: `place` is heap-pinned, outlives this scope, and is not
                    // touched while the endpoints borrow it. The endpoints do not escape
                    // the scope, so they are gone before the storage is returned. The
                    // place was freshly created and is not in use by another event.
                    let (sender, receiver) = black_box(unsafe { Event::placed(place.as_mut()) });
                    let mut receiver = pin!(receiver);

                    _ = black_box(receiver.as_mut().poll(&mut cx));

                    sender.send(black_box(PAYLOAD));

                    _ = black_box(receiver.as_mut().poll(&mut cx));
                }

                place
            },
            BatchSize::SmallInput,
        );
    });

    bench_lifecycle_await_first!(g, "local/pooled", local_pool.rent());
    bench_lifecycle_await_first!(g, "sync/pooled", sync_pool.rent());

    // SAFETY (raw pool and lake rows): the pool or lake outlives every rented endpoint
    // because it is created before the benchmark group and the endpoints never escape
    // the closure that rents them.
    bench_lifecycle_await_first!(g, "local/raw_pooled", unsafe {
        local_raw_pool.as_ref().rent()
    });
    bench_lifecycle_await_first!(g, "sync/raw_pooled", unsafe {
        sync_raw_pool.as_ref().rent()
    });

    bench_lifecycle_await_first!(g, "local/lake", local_lake.rent::<i32>());
    bench_lifecycle_await_first!(g, "sync/lake", sync_lake.rent::<i32>());

    bench_lifecycle_await_first!(g, "local/raw_lake", unsafe { local_raw_lake.rent::<i32>() });
    bench_lifecycle_await_first!(g, "sync/raw_lake", unsafe { sync_raw_lake.rent::<i32>() });

    g.bench_function("oneshot", |b| {
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

// Focused send: only the send call is measured, with the peer prepared in a named state
// beforehand. Boxed storage stands in for every strategy here because the measured
// region touches storage only in the disconnected case; the sweep over
// storage-specific release paths lives in the `cancel` group.
fn send(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/send");

    g.bench_function("local/bound", |b| {
        b.iter_batched(
            local_boxed_bound,
            |(sender, receiver)| {
                sender.send(black_box(PAYLOAD));
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/bound", |b| {
        b.iter_batched(
            sync_boxed_bound,
            |(sender, receiver)| {
                sender.send(black_box(PAYLOAD));
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("local/awaiting", |b| {
        b.iter_batched(
            local_boxed_awaiting,
            |(sender, receiver)| {
                sender.send(black_box(PAYLOAD));
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/awaiting", |b| {
        b.iter_batched(
            sync_boxed_awaiting,
            |(sender, receiver)| {
                sender.send(black_box(PAYLOAD));
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    // With the receiver already gone, the send drops the payload and releases the event
    // storage, so the measured region includes that release.
    g.bench_function("local/disconnected", |b| {
        b.iter_batched(
            local_boxed_sender_only,
            |sender| sender.send(black_box(PAYLOAD)),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/disconnected", |b| {
        b.iter_batched(
            sync_boxed_sender_only,
            |sender| sender.send(black_box(PAYLOAD)),
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// Focused poll, on boxed storage for the same reason as the send group. The polling
// context is built before `iter_batched`, so no iteration pays for it.
fn poll(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/poll");

    g.bench_function("local/pending_first", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            local_boxed_bound,
            |(sender, mut receiver)| {
                _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
                (sender, receiver)
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/pending_first", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            sync_boxed_bound,
            |(sender, mut receiver)| {
                _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
                (sender, receiver)
            },
            BatchSize::SmallInput,
        );
    });

    // Re-polling an event that already holds a waker is the common shape for a task that
    // is woken for an unrelated reason and polls all its futures again.
    g.bench_function("local/pending_repeat", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            local_boxed_awaiting,
            |(sender, mut receiver)| {
                _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
                (sender, receiver)
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/pending_repeat", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            sync_boxed_awaiting,
            |(sender, mut receiver)| {
                _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
                (sender, receiver)
            },
            BatchSize::SmallInput,
        );
    });

    // The poll observes the terminal disconnected state and releases the storage, so the
    // release is part of the measured region.
    g.bench_function("local/disconnected", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            local_boxed_disconnected,
            |mut receiver| {
                _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/disconnected", |b| {
        let mut cx = noop_context();

        b.iter_batched(
            sync_boxed_disconnected,
            |mut receiver| {
                _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// Synchronous value extraction. The three start states select different work: a pending
// event hands the receiver back untouched, whereas the two terminal states resolve the
// outcome and release the storage inside the measured region.
//
// The case names follow the receiver API vocabulary: `ready` is the state the state
// machine calls `set` and `pending` is the state it calls `bound` (see
// `src/core/state.rs`).
fn into_value(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/into_value");

    g.bench_function("local/pending", |b| {
        b.iter_batched(
            local_boxed_bound,
            |(sender, receiver)| (sender, black_box(receiver.into_value())),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/pending", |b| {
        b.iter_batched(
            sync_boxed_bound,
            |(sender, receiver)| (sender, black_box(receiver.into_value())),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("local/ready", |b| {
        b.iter_batched(
            local_boxed_set,
            |receiver| black_box(receiver.into_value()),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/ready", |b| {
        b.iter_batched(
            sync_boxed_set,
            |receiver| black_box(receiver.into_value()),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("local/disconnected", |b| {
        b.iter_batched(
            local_boxed_disconnected,
            |receiver| black_box(receiver.into_value()),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/disconnected", |b| {
        b.iter_batched(
            sync_boxed_disconnected,
            |receiver| black_box(receiver.into_value()),
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// Readiness probing, over the same three start states as `into_value`. This is the
// cheapest public receiver operation - a single state read - so it is measured on real
// hardware only: an instruction count at this magnitude is dominated by the harness
// rather than by the operation.
fn is_ready(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/is_ready");

    g.bench_function("local/pending", |b| {
        b.iter_batched(
            local_boxed_bound,
            |(sender, receiver)| {
                _ = black_box(receiver.is_ready());
                (sender, receiver)
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/pending", |b| {
        b.iter_batched(
            sync_boxed_bound,
            |(sender, receiver)| {
                _ = black_box(receiver.is_ready());
                (sender, receiver)
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("local/ready", |b| {
        b.iter_batched(
            local_boxed_set,
            |receiver| {
                _ = black_box(receiver.is_ready());
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/ready", |b| {
        b.iter_batched(
            sync_boxed_set,
            |receiver| {
                _ = black_box(receiver.is_ready());
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("local/disconnected", |b| {
        b.iter_batched(
            local_boxed_disconnected,
            |receiver| {
                _ = black_box(receiver.is_ready());
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sync/disconnected", |b| {
        b.iter_batched(
            sync_boxed_disconnected,
            |receiver| {
                _ = black_box(receiver.is_ready());
                receiver
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// Cancellation: no value is ever delivered. The measured region covers both endpoint
// drops, so it includes the storage-specific final release performed by whichever
// endpoint goes last, and every storage strategy is swept because release is exactly
// what differs between them. Whatever owns the storage - a pool handle, a lake handle,
// or the caller's embedded place - is returned from the measured closure so that its
// own teardown stays outside the measured region.
//
// Four start states are measured per storage and model:
//
// - `sender_first_bound`: the receiver never polled, so there is no waker to consume.
// - `sender_first_awaiting`: the receiver parked a waker, which the sender consumes and
//   wakes; the state machine that both variants implement is defined in
//   `src/core/state.rs` and described in `docs/implementation.md`.
// - `receiver_first_bound`: the receiver is dropped before polling, which makes the
//   sender the endpoint that releases the storage.
// - `receiver_first_awaiting`: the receiver is dropped after parking a waker, so receiver
//   cancellation includes the waker cleanup and state handoff before the sender releases
//   the storage.
fn cancel(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_ops/cancel");

    bench_cancel_boxed!(
        g,
        "local/boxed/sender_first_bound",
        local_boxed_bound,
        sender_first
    );
    bench_cancel_boxed!(
        g,
        "sync/boxed/sender_first_bound",
        sync_boxed_bound,
        sender_first
    );
    bench_cancel_boxed!(
        g,
        "local/boxed/sender_first_awaiting",
        local_boxed_awaiting,
        sender_first
    );
    bench_cancel_boxed!(
        g,
        "sync/boxed/sender_first_awaiting",
        sync_boxed_awaiting,
        sender_first
    );
    bench_cancel_boxed!(
        g,
        "local/boxed/receiver_first_bound",
        local_boxed_bound,
        receiver_first
    );
    bench_cancel_boxed!(
        g,
        "sync/boxed/receiver_first_bound",
        sync_boxed_bound,
        receiver_first
    );
    bench_cancel_boxed!(
        g,
        "local/boxed/receiver_first_awaiting",
        local_boxed_awaiting,
        receiver_first
    );
    bench_cancel_boxed!(
        g,
        "sync/boxed/receiver_first_awaiting",
        sync_boxed_awaiting,
        receiver_first
    );

    bench_cancel_owned!(
        g,
        "local/embedded/sender_first_bound",
        local_embedded_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/embedded/sender_first_bound",
        sync_embedded_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/embedded/sender_first_awaiting",
        local_embedded_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/embedded/sender_first_awaiting",
        sync_embedded_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/embedded/receiver_first_bound",
        local_embedded_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/embedded/receiver_first_bound",
        sync_embedded_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "local/embedded/receiver_first_awaiting",
        local_embedded_awaiting,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/embedded/receiver_first_awaiting",
        sync_embedded_awaiting,
        receiver_first
    );

    bench_cancel_owned!(
        g,
        "local/pooled/sender_first_bound",
        local_pooled_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/pooled/sender_first_bound",
        sync_pooled_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/pooled/sender_first_awaiting",
        local_pooled_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/pooled/sender_first_awaiting",
        sync_pooled_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/pooled/receiver_first_bound",
        local_pooled_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/pooled/receiver_first_bound",
        sync_pooled_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "local/pooled/receiver_first_awaiting",
        local_pooled_awaiting,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/pooled/receiver_first_awaiting",
        sync_pooled_awaiting,
        receiver_first
    );

    bench_cancel_owned!(
        g,
        "local/raw_pooled/sender_first_bound",
        local_raw_pooled_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_pooled/sender_first_bound",
        sync_raw_pooled_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/raw_pooled/sender_first_awaiting",
        local_raw_pooled_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_pooled/sender_first_awaiting",
        sync_raw_pooled_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/raw_pooled/receiver_first_bound",
        local_raw_pooled_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_pooled/receiver_first_bound",
        sync_raw_pooled_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "local/raw_pooled/receiver_first_awaiting",
        local_raw_pooled_awaiting,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_pooled/receiver_first_awaiting",
        sync_raw_pooled_awaiting,
        receiver_first
    );

    bench_cancel_owned!(
        g,
        "local/lake/sender_first_bound",
        local_lake_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/lake/sender_first_bound",
        sync_lake_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/lake/sender_first_awaiting",
        local_lake_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/lake/sender_first_awaiting",
        sync_lake_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/lake/receiver_first_bound",
        local_lake_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/lake/receiver_first_bound",
        sync_lake_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "local/lake/receiver_first_awaiting",
        local_lake_awaiting,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/lake/receiver_first_awaiting",
        sync_lake_awaiting,
        receiver_first
    );

    bench_cancel_owned!(
        g,
        "local/raw_lake/sender_first_bound",
        local_raw_lake_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_lake/sender_first_bound",
        sync_raw_lake_bound,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/raw_lake/sender_first_awaiting",
        local_raw_lake_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_lake/sender_first_awaiting",
        sync_raw_lake_awaiting,
        sender_first
    );
    bench_cancel_owned!(
        g,
        "local/raw_lake/receiver_first_bound",
        local_raw_lake_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_lake/receiver_first_bound",
        sync_raw_lake_bound,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "local/raw_lake/receiver_first_awaiting",
        local_raw_lake_awaiting,
        receiver_first
    );
    bench_cancel_owned!(
        g,
        "sync/raw_lake/receiver_first_awaiting",
        sync_raw_lake_awaiting,
        receiver_first
    );

    g.finish();
}

fn entrypoint(c: &mut Criterion) {
    rent(c);
    lifecycle(c);
    lifecycle_await_first(c);
    send(c);
    poll(c);
    into_value(c);
    is_ready(c);
    cancel(c);
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
