//! Callgrind (instruction-count) half of the canonical benchmark scenario matrix of
//! the `events_once` package.
//!
//! Paired with `events_once_ops.rs`, which measures the same scenarios on real
//! hardware. The scenario matrix itself - which fixes, for every row, the threading
//! model, storage strategy, start state, timed operation and cleanup boundary - is
//! documented in `packages/events_once/AGENTS.md` ("Canonical benchmark scenario
//! matrix"). Every row here has an identically prepared Criterion twin there, under
//! the same group name with `_` in place of `/`.
//!
//! Everything that is not the operation under test is prepared by a setup function,
//! which Gungraun evaluates outside the measured region: endpoints, warmed pools and
//! lakes, caller-owned embedded storage, and the noop-waker polling context.
//!
//! Callgrind attributes a change to a code path; it does not establish real
//! execution speed. Conclusions about speed - including the requirement that the
//! single-threaded variant stays at least as fast as the thread-safe one - are drawn
//! from the paired Criterion group. See `docs/callgrind-benchmarks.md`,
//! "Cross-validate design decisions against Criterion".

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
    use std::pin::{Pin, pin};
    use std::task::{self, Waker};

    use events_once::{
        BoxedLocalReceiver, BoxedLocalSender, BoxedReceiver, BoxedSender, EmbeddedEvent,
        EmbeddedLocalEvent, Event, EventLake, EventPool, IntoValueError, LocalEvent,
        LocalEventLake, LocalEventPool, PooledLocalReceiver, PooledLocalSender, PooledReceiver,
        PooledSender, RawEventLake, RawEventPool, RawLocalEventLake, RawLocalEventPool,
        RawLocalPooledReceiver, RawLocalPooledSender, RawLocalReceiver, RawLocalSender,
        RawPooledReceiver, RawPooledSender, RawReceiver, RawSender,
    };
    use gungraun::prelude::*;

    // Arbitrary payload. The event machinery treats the payload as opaque, so the
    // specific value only needs to be stable across scenarios to keep them comparable.
    const PAYLOAD: i32 = 42;

    // A polling context built on the static noop waker. It carries no per-event state,
    // so one prepared instance serves any number of polls and is prepared by setup.
    type NoopContext = task::Context<'static>;

    type LocalBoxedEndpoints = (BoxedLocalSender<i32>, BoxedLocalReceiver<i32>);
    type SyncBoxedEndpoints = (BoxedSender<i32>, BoxedReceiver<i32>);
    type LocalEmbeddedEndpoints = (RawLocalSender<i32>, RawLocalReceiver<i32>);
    type SyncEmbeddedEndpoints = (RawSender<i32>, RawReceiver<i32>);
    type LocalPooledEndpoints = (PooledLocalSender<i32>, PooledLocalReceiver<i32>);
    type SyncPooledEndpoints = (PooledSender<i32>, PooledReceiver<i32>);
    type LocalRawPooledEndpoints = (RawLocalPooledSender<i32>, RawLocalPooledReceiver<i32>);
    type SyncRawPooledEndpoints = (RawPooledSender<i32>, RawPooledReceiver<i32>);

    // Caller-owned storage. The event is placed into it and released from it, but the
    // storage itself belongs to the benchmark, not to the event, which is what makes it
    // preparable outside the measured region. Heap pinning is what lets it cross a
    // function boundary; it is not the boxed-event strategy, where the event owns its
    // own allocation and frees it on release.
    type LocalEmbeddedStorage = Pin<Box<EmbeddedLocalEvent<i32>>>;
    type SyncEmbeddedStorage = Pin<Box<EmbeddedEvent<i32>>>;
    type LocalRawPool = Pin<Box<RawLocalEventPool<i32>>>;
    type SyncRawPool = Pin<Box<RawEventPool<i32>>>;

    type LocalIntoValueResult = Result<i32, IntoValueError<BoxedLocalReceiver<i32>>>;
    type SyncIntoValueResult = Result<i32, IntoValueError<BoxedReceiver<i32>>>;

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

    // Storage warm-up: rent one event and return it, so a measured region that rents
    // works against a pool that already owns a recycled event and does not allocate.

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

    // Inputs for the focused operations, which start from a state that a peer endpoint
    // has already left behind.

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

    // Steady-state rental from warmed managed pools. Returning the pool and endpoints keeps every
    // destructor outside the measured region, so the count isolates slot acquisition, event
    // initialization and endpoint construction.

    #[library_benchmark]
    #[bench::warm(warm_local_pool())]
    fn rent_local_pooled(pool: LocalEventPool<i32>) -> (LocalEventPool<i32>, LocalPooledEndpoints) {
        let endpoints = black_box(pool.rent());
        black_box((pool, endpoints))
    }

    #[library_benchmark]
    #[bench::warm(warm_sync_pool())]
    fn rent_sync_pooled(pool: EventPool<i32>) -> (EventPool<i32>, SyncPooledEndpoints) {
        let endpoints = black_box(pool.rent());
        black_box((pool, endpoints))
    }

    // Send-first lifecycle: acquire the endpoints, send, poll out the value and release
    // the storage - all inside the measured region.
    //
    // The receiver is stack-pinned via `pin!` rather than `Box::pin` so that the
    // measured iteration reflects event mechanics rather than allocator overhead; see
    // `docs/benchmarks.md` ("Stack pin vs. `Box::pin` on the measured path").
    //
    // The boxed rows allocate the event on acquisition and free it on release, so their
    // counts include the instructions spent in the allocator call, but Callgrind does
    // not reproduce real allocator, cache or operating-system latency. Rank storage
    // strategies against each other using the paired Criterion group, which measures
    // that cost on real hardware; use these counts only to attribute a change to a
    // code path.
    //
    // The embedded rows acquire by placing the event into caller-owned storage that
    // setup prepared, so their measured region contains placement and release but no
    // storage allocation. That is the difference between embedding an event in an
    // object the caller already owns and letting the event own its own allocation.

    #[library_benchmark]
    #[bench::fresh(noop_context())]
    fn lifecycle_local_boxed(cx: NoopContext) {
        // Rebound rather than taken as a `mut` parameter: a `mut` binding in the
        // signature is a pattern, which the benchmark macro is not obliged to preserve.
        let mut cx = cx;

        let (sender, receiver) = black_box(LocalEvent::<i32>::boxed());
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    #[library_benchmark]
    #[bench::fresh(noop_context())]
    fn lifecycle_sync_boxed(cx: NoopContext) {
        // Rebound rather than taken as a `mut` parameter: a `mut` binding in the
        // signature is a pattern, which the benchmark macro is not obliged to preserve.
        let mut cx = cx;

        let (sender, receiver) = black_box(Event::<i32>::boxed());
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));
    }

    #[library_benchmark]
    #[bench::fresh((local_embedded_storage(), noop_context()))]
    fn lifecycle_local_embedded(
        input: (LocalEmbeddedStorage, NoopContext),
    ) -> LocalEmbeddedStorage {
        let (mut place, mut cx) = input;

        {
            // SAFETY: `place` is heap-pinned, outlives this scope, and is not touched while
            // the endpoints borrow it. The endpoints do not escape the scope, so they are
            // gone before the storage is returned. The place was freshly created by setup
            // and is not in use by another event.
            let (sender, receiver) = black_box(unsafe { LocalEvent::placed(place.as_mut()) });
            let mut receiver = pin!(receiver);

            sender.send(black_box(PAYLOAD));

            _ = black_box(receiver.as_mut().poll(&mut cx));
        }

        place
    }

    #[library_benchmark]
    #[bench::fresh((sync_embedded_storage(), noop_context()))]
    fn lifecycle_sync_embedded(input: (SyncEmbeddedStorage, NoopContext)) -> SyncEmbeddedStorage {
        let (mut place, mut cx) = input;

        {
            // SAFETY: `place` is heap-pinned, outlives this scope, and is not touched while
            // the endpoints borrow it. The endpoints do not escape the scope, so they are
            // gone before the storage is returned. The place was freshly created by setup
            // and is not in use by another event.
            let (sender, receiver) = black_box(unsafe { Event::placed(place.as_mut()) });
            let mut receiver = pin!(receiver);

            sender.send(black_box(PAYLOAD));

            _ = black_box(receiver.as_mut().poll(&mut cx));
        }

        place
    }

    #[library_benchmark]
    #[bench::warm((warm_local_pool(), noop_context()))]
    fn lifecycle_local_pooled(input: (LocalEventPool<i32>, NoopContext)) -> LocalEventPool<i32> {
        let (pool, mut cx) = input;

        let (sender, receiver) = black_box(pool.rent());
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    #[library_benchmark]
    #[bench::warm((warm_sync_pool(), noop_context()))]
    fn lifecycle_sync_pooled(input: (EventPool<i32>, NoopContext)) -> EventPool<i32> {
        let (pool, mut cx) = input;

        let (sender, receiver) = black_box(pool.rent());
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    #[library_benchmark]
    #[bench::warm((warm_local_raw_pool(), noop_context()))]
    fn lifecycle_local_raw_pooled(input: (LocalRawPool, NoopContext)) -> LocalRawPool {
        let (pool, mut cx) = input;

        // SAFETY: the endpoints do not escape this function, so they cannot outlive the
        // pool that is returned from it.
        let (sender, receiver) = black_box(unsafe { pool.as_ref().rent() });
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    #[library_benchmark]
    #[bench::warm((warm_sync_raw_pool(), noop_context()))]
    fn lifecycle_sync_raw_pooled(input: (SyncRawPool, NoopContext)) -> SyncRawPool {
        let (pool, mut cx) = input;

        // SAFETY: the endpoints do not escape this function, so they cannot outlive the
        // pool that is returned from it.
        let (sender, receiver) = black_box(unsafe { pool.as_ref().rent() });
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    #[library_benchmark]
    #[bench::warm((warm_local_lake(), noop_context()))]
    fn lifecycle_local_lake(input: (LocalEventLake, NoopContext)) -> LocalEventLake {
        let (lake, mut cx) = input;

        let (sender, receiver) = black_box(lake.rent::<i32>());
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        lake
    }

    #[library_benchmark]
    #[bench::warm((warm_sync_lake(), noop_context()))]
    fn lifecycle_sync_lake(input: (EventLake, NoopContext)) -> EventLake {
        let (lake, mut cx) = input;

        let (sender, receiver) = black_box(lake.rent::<i32>());
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        lake
    }

    #[library_benchmark]
    #[bench::warm((warm_local_raw_lake(), noop_context()))]
    fn lifecycle_local_raw_lake(input: (RawLocalEventLake, NoopContext)) -> RawLocalEventLake {
        let (lake, mut cx) = input;

        // SAFETY: the endpoints do not escape this function, so they cannot outlive the
        // lake that is returned from it.
        let (sender, receiver) = black_box(unsafe { lake.rent::<i32>() });
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        lake
    }

    #[library_benchmark]
    #[bench::warm((warm_sync_raw_lake(), noop_context()))]
    fn lifecycle_sync_raw_lake(input: (RawEventLake, NoopContext)) -> RawEventLake {
        let (lake, mut cx) = input;

        // SAFETY: the endpoints do not escape this function, so they cannot outlive the
        // lake that is returned from it.
        let (sender, receiver) = black_box(unsafe { lake.rent::<i32>() });
        let mut receiver = pin!(receiver);

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        lake
    }

    // Await-first lifecycle: the receiver polls (and parks a waker) before the value is
    // sent, so delivery runs through the awaiting state and wakes the receiver.
    //
    // Only the pooled storage is measured here. What distinguishes this lifecycle from
    // the send-first one lives entirely in the event state machine, which is shared by
    // every storage strategy, so repeating the storage sweep would re-measure the
    // acquisition and release paths that the `lifecycle` group already covers. Pooled
    // storage is the package's primary performance target. The Criterion twin does
    // sweep the storage strategies, because that group also hosts the third-party
    // comparison.

    #[library_benchmark]
    #[bench::warm((warm_local_pool(), noop_context()))]
    fn lifecycle_await_first_local_pooled(
        input: (LocalEventPool<i32>, NoopContext),
    ) -> LocalEventPool<i32> {
        let (pool, mut cx) = input;

        let (sender, receiver) = black_box(pool.rent());
        let mut receiver = pin!(receiver);

        _ = black_box(receiver.as_mut().poll(&mut cx));

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    #[library_benchmark]
    #[bench::warm((warm_sync_pool(), noop_context()))]
    fn lifecycle_await_first_sync_pooled(input: (EventPool<i32>, NoopContext)) -> EventPool<i32> {
        let (pool, mut cx) = input;

        let (sender, receiver) = black_box(pool.rent());
        let mut receiver = pin!(receiver);

        _ = black_box(receiver.as_mut().poll(&mut cx));

        sender.send(black_box(PAYLOAD));

        _ = black_box(receiver.as_mut().poll(&mut cx));

        pool
    }

    // Focused send: only the send call is measured, with the peer prepared in a named
    // state beforehand. Boxed storage stands in for every strategy here because the
    // measured region touches storage only in the disconnected case; the sweep over
    // storage-specific release paths lives in the `cancel` group.

    #[library_benchmark]
    #[bench::bound(local_boxed_bound())]
    fn send_local_bound(input: LocalBoxedEndpoints) -> BoxedLocalReceiver<i32> {
        let (sender, receiver) = input;
        sender.send(black_box(PAYLOAD));
        receiver
    }

    #[library_benchmark]
    #[bench::bound(sync_boxed_bound())]
    fn send_sync_bound(input: SyncBoxedEndpoints) -> BoxedReceiver<i32> {
        let (sender, receiver) = input;
        sender.send(black_box(PAYLOAD));
        receiver
    }

    #[library_benchmark]
    #[bench::awaiting(local_boxed_awaiting())]
    fn send_local_awaiting(input: LocalBoxedEndpoints) -> BoxedLocalReceiver<i32> {
        let (sender, receiver) = input;
        sender.send(black_box(PAYLOAD));
        receiver
    }

    #[library_benchmark]
    #[bench::awaiting(sync_boxed_awaiting())]
    fn send_sync_awaiting(input: SyncBoxedEndpoints) -> BoxedReceiver<i32> {
        let (sender, receiver) = input;
        sender.send(black_box(PAYLOAD));
        receiver
    }

    // With the receiver already gone, the send drops the payload and releases the event
    // storage, so the measured region includes that release.

    #[library_benchmark]
    #[bench::disconnected(local_boxed_sender_only())]
    fn send_local_disconnected(sender: BoxedLocalSender<i32>) {
        sender.send(black_box(PAYLOAD));
    }

    #[library_benchmark]
    #[bench::disconnected(sync_boxed_sender_only())]
    fn send_sync_disconnected(sender: BoxedSender<i32>) {
        sender.send(black_box(PAYLOAD));
    }

    // Focused poll, on boxed storage for the same reason as the send group. The polling
    // context arrives from setup, so no row pays for building one.

    #[library_benchmark]
    #[bench::bound((local_boxed_bound(), noop_context()))]
    fn poll_local_pending_first(input: (LocalBoxedEndpoints, NoopContext)) -> LocalBoxedEndpoints {
        let ((sender, mut receiver), mut cx) = input;
        _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
        (sender, receiver)
    }

    #[library_benchmark]
    #[bench::bound((sync_boxed_bound(), noop_context()))]
    fn poll_sync_pending_first(input: (SyncBoxedEndpoints, NoopContext)) -> SyncBoxedEndpoints {
        let ((sender, mut receiver), mut cx) = input;
        _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
        (sender, receiver)
    }

    // Re-polling an event that already holds a waker is the common shape for a task that
    // is woken for an unrelated reason and polls all its futures again.

    #[library_benchmark]
    #[bench::awaiting((local_boxed_awaiting(), noop_context()))]
    fn poll_local_pending_repeat(input: (LocalBoxedEndpoints, NoopContext)) -> LocalBoxedEndpoints {
        let ((sender, mut receiver), mut cx) = input;
        _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
        (sender, receiver)
    }

    #[library_benchmark]
    #[bench::awaiting((sync_boxed_awaiting(), noop_context()))]
    fn poll_sync_pending_repeat(input: (SyncBoxedEndpoints, NoopContext)) -> SyncBoxedEndpoints {
        let ((sender, mut receiver), mut cx) = input;
        _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
        (sender, receiver)
    }

    // The poll observes the terminal disconnected state and releases the storage, so the
    // release is part of the measured region.

    #[library_benchmark]
    #[bench::disconnected((local_boxed_disconnected(), noop_context()))]
    fn poll_local_disconnected(
        input: (BoxedLocalReceiver<i32>, NoopContext),
    ) -> BoxedLocalReceiver<i32> {
        let (mut receiver, mut cx) = input;
        _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
        receiver
    }

    #[library_benchmark]
    #[bench::disconnected((sync_boxed_disconnected(), noop_context()))]
    fn poll_sync_disconnected(input: (BoxedReceiver<i32>, NoopContext)) -> BoxedReceiver<i32> {
        let (mut receiver, mut cx) = input;
        _ = black_box(Pin::new(&mut receiver).poll(&mut cx));
        receiver
    }

    // Synchronous value extraction. The three start states select different work: a
    // pending event hands the receiver back untouched, whereas the two terminal states
    // resolve the outcome and release the storage inside the measured region.
    //
    // The case names follow the receiver API vocabulary: `ready` is the state the state
    // machine calls `set` and `pending` is the state it calls `bound` (see
    // `src/core/state.rs`).

    #[library_benchmark]
    #[bench::pending(local_boxed_bound())]
    fn into_value_local_pending(
        input: LocalBoxedEndpoints,
    ) -> (BoxedLocalSender<i32>, LocalIntoValueResult) {
        let (sender, receiver) = input;
        (sender, black_box(receiver.into_value()))
    }

    #[library_benchmark]
    #[bench::pending(sync_boxed_bound())]
    fn into_value_sync_pending(
        input: SyncBoxedEndpoints,
    ) -> (BoxedSender<i32>, SyncIntoValueResult) {
        let (sender, receiver) = input;
        (sender, black_box(receiver.into_value()))
    }

    #[library_benchmark]
    #[bench::ready(local_boxed_set())]
    fn into_value_local_ready(receiver: BoxedLocalReceiver<i32>) -> LocalIntoValueResult {
        black_box(receiver.into_value())
    }

    #[library_benchmark]
    #[bench::ready(sync_boxed_set())]
    fn into_value_sync_ready(receiver: BoxedReceiver<i32>) -> SyncIntoValueResult {
        black_box(receiver.into_value())
    }

    #[library_benchmark]
    #[bench::disconnected(local_boxed_disconnected())]
    fn into_value_local_disconnected(receiver: BoxedLocalReceiver<i32>) -> LocalIntoValueResult {
        black_box(receiver.into_value())
    }

    #[library_benchmark]
    #[bench::disconnected(sync_boxed_disconnected())]
    fn into_value_sync_disconnected(receiver: BoxedReceiver<i32>) -> SyncIntoValueResult {
        black_box(receiver.into_value())
    }

    // Cancellation: no value is ever delivered. The measured region covers both endpoint
    // drops, so it includes the storage-specific final release performed by whichever
    // endpoint goes last, and every storage strategy is swept because release is exactly
    // what differs between them. Whatever owns the storage - a pool handle, a lake
    // handle, or the caller's embedded place - is returned from the measured region so
    // that its own teardown stays untimed.
    //
    // Three start states are measured per storage and model:
    //
    // - `sender_first_bound`: the receiver never polled, so there is no waker to consume.
    // - `sender_first_awaiting`: the receiver parked a waker, which the sender consumes
    //   and wakes. Both variants publish the terminal disconnected state before the wake
    //   runs, so a waker that re-polls the receiver inline observes a completed event;
    //   the thread-safe variant first passes through the signaling state to take
    //   exclusive ownership of the waker, which the single-threaded variant does not
    //   need. The state machine itself is defined in `src/core/state.rs` and described
    //   in `docs/implementation.md` ("The state machine is the single source of truth").
    // - `receiver_first_bound`: the receiver is dropped before polling, which makes the
    //   sender the endpoint that releases the storage.

    #[library_benchmark]
    #[bench::bound(local_boxed_bound())]
    fn cancel_local_boxed_sender_first_bound(input: LocalBoxedEndpoints) {
        let (sender, receiver) = input;
        drop(sender);
        drop(receiver);
    }

    #[library_benchmark]
    #[bench::bound(sync_boxed_bound())]
    fn cancel_sync_boxed_sender_first_bound(input: SyncBoxedEndpoints) {
        let (sender, receiver) = input;
        drop(sender);
        drop(receiver);
    }

    #[library_benchmark]
    #[bench::awaiting(local_boxed_awaiting())]
    fn cancel_local_boxed_sender_first_awaiting(input: LocalBoxedEndpoints) {
        let (sender, receiver) = input;
        drop(sender);
        drop(receiver);
    }

    #[library_benchmark]
    #[bench::awaiting(sync_boxed_awaiting())]
    fn cancel_sync_boxed_sender_first_awaiting(input: SyncBoxedEndpoints) {
        let (sender, receiver) = input;
        drop(sender);
        drop(receiver);
    }

    #[library_benchmark]
    #[bench::bound(local_boxed_bound())]
    fn cancel_local_boxed_receiver_first_bound(input: LocalBoxedEndpoints) {
        let (sender, receiver) = input;
        drop(receiver);
        drop(sender);
    }

    #[library_benchmark]
    #[bench::bound(sync_boxed_bound())]
    fn cancel_sync_boxed_receiver_first_bound(input: SyncBoxedEndpoints) {
        let (sender, receiver) = input;
        drop(receiver);
        drop(sender);
    }

    #[library_benchmark]
    #[bench::bound(local_embedded_bound())]
    fn cancel_local_embedded_sender_first_bound(
        input: (LocalEmbeddedStorage, LocalEmbeddedEndpoints),
    ) -> LocalEmbeddedStorage {
        let (place, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        place
    }

    #[library_benchmark]
    #[bench::bound(sync_embedded_bound())]
    fn cancel_sync_embedded_sender_first_bound(
        input: (SyncEmbeddedStorage, SyncEmbeddedEndpoints),
    ) -> SyncEmbeddedStorage {
        let (place, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        place
    }

    #[library_benchmark]
    #[bench::awaiting(local_embedded_awaiting())]
    fn cancel_local_embedded_sender_first_awaiting(
        input: (LocalEmbeddedStorage, LocalEmbeddedEndpoints),
    ) -> LocalEmbeddedStorage {
        let (place, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        place
    }

    #[library_benchmark]
    #[bench::awaiting(sync_embedded_awaiting())]
    fn cancel_sync_embedded_sender_first_awaiting(
        input: (SyncEmbeddedStorage, SyncEmbeddedEndpoints),
    ) -> SyncEmbeddedStorage {
        let (place, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        place
    }

    #[library_benchmark]
    #[bench::bound(local_embedded_bound())]
    fn cancel_local_embedded_receiver_first_bound(
        input: (LocalEmbeddedStorage, LocalEmbeddedEndpoints),
    ) -> LocalEmbeddedStorage {
        let (place, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        place
    }

    #[library_benchmark]
    #[bench::bound(sync_embedded_bound())]
    fn cancel_sync_embedded_receiver_first_bound(
        input: (SyncEmbeddedStorage, SyncEmbeddedEndpoints),
    ) -> SyncEmbeddedStorage {
        let (place, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        place
    }

    #[library_benchmark]
    #[bench::bound(local_pooled_bound())]
    fn cancel_local_pooled_sender_first_bound(
        input: (LocalEventPool<i32>, LocalPooledEndpoints),
    ) -> LocalEventPool<i32> {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::bound(sync_pooled_bound())]
    fn cancel_sync_pooled_sender_first_bound(
        input: (EventPool<i32>, SyncPooledEndpoints),
    ) -> EventPool<i32> {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::awaiting(local_pooled_awaiting())]
    fn cancel_local_pooled_sender_first_awaiting(
        input: (LocalEventPool<i32>, LocalPooledEndpoints),
    ) -> LocalEventPool<i32> {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::awaiting(sync_pooled_awaiting())]
    fn cancel_sync_pooled_sender_first_awaiting(
        input: (EventPool<i32>, SyncPooledEndpoints),
    ) -> EventPool<i32> {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::bound(local_pooled_bound())]
    fn cancel_local_pooled_receiver_first_bound(
        input: (LocalEventPool<i32>, LocalPooledEndpoints),
    ) -> LocalEventPool<i32> {
        let (pool, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        pool
    }

    #[library_benchmark]
    #[bench::bound(sync_pooled_bound())]
    fn cancel_sync_pooled_receiver_first_bound(
        input: (EventPool<i32>, SyncPooledEndpoints),
    ) -> EventPool<i32> {
        let (pool, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        pool
    }

    #[library_benchmark]
    #[bench::bound(local_raw_pooled_bound())]
    fn cancel_local_raw_pooled_sender_first_bound(
        input: (LocalRawPool, LocalRawPooledEndpoints),
    ) -> LocalRawPool {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::bound(sync_raw_pooled_bound())]
    fn cancel_sync_raw_pooled_sender_first_bound(
        input: (SyncRawPool, SyncRawPooledEndpoints),
    ) -> SyncRawPool {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::awaiting(local_raw_pooled_awaiting())]
    fn cancel_local_raw_pooled_sender_first_awaiting(
        input: (LocalRawPool, LocalRawPooledEndpoints),
    ) -> LocalRawPool {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::awaiting(sync_raw_pooled_awaiting())]
    fn cancel_sync_raw_pooled_sender_first_awaiting(
        input: (SyncRawPool, SyncRawPooledEndpoints),
    ) -> SyncRawPool {
        let (pool, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        pool
    }

    #[library_benchmark]
    #[bench::bound(local_raw_pooled_bound())]
    fn cancel_local_raw_pooled_receiver_first_bound(
        input: (LocalRawPool, LocalRawPooledEndpoints),
    ) -> LocalRawPool {
        let (pool, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        pool
    }

    #[library_benchmark]
    #[bench::bound(sync_raw_pooled_bound())]
    fn cancel_sync_raw_pooled_receiver_first_bound(
        input: (SyncRawPool, SyncRawPooledEndpoints),
    ) -> SyncRawPool {
        let (pool, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        pool
    }

    #[library_benchmark]
    #[bench::bound(local_lake_bound())]
    fn cancel_local_lake_sender_first_bound(
        input: (LocalEventLake, LocalPooledEndpoints),
    ) -> LocalEventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::bound(sync_lake_bound())]
    fn cancel_sync_lake_sender_first_bound(input: (EventLake, SyncPooledEndpoints)) -> EventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::awaiting(local_lake_awaiting())]
    fn cancel_local_lake_sender_first_awaiting(
        input: (LocalEventLake, LocalPooledEndpoints),
    ) -> LocalEventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::awaiting(sync_lake_awaiting())]
    fn cancel_sync_lake_sender_first_awaiting(
        input: (EventLake, SyncPooledEndpoints),
    ) -> EventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::bound(local_lake_bound())]
    fn cancel_local_lake_receiver_first_bound(
        input: (LocalEventLake, LocalPooledEndpoints),
    ) -> LocalEventLake {
        let (lake, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        lake
    }

    #[library_benchmark]
    #[bench::bound(sync_lake_bound())]
    fn cancel_sync_lake_receiver_first_bound(input: (EventLake, SyncPooledEndpoints)) -> EventLake {
        let (lake, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        lake
    }

    #[library_benchmark]
    #[bench::bound(local_raw_lake_bound())]
    fn cancel_local_raw_lake_sender_first_bound(
        input: (RawLocalEventLake, LocalRawPooledEndpoints),
    ) -> RawLocalEventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::bound(sync_raw_lake_bound())]
    fn cancel_sync_raw_lake_sender_first_bound(
        input: (RawEventLake, SyncRawPooledEndpoints),
    ) -> RawEventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::awaiting(local_raw_lake_awaiting())]
    fn cancel_local_raw_lake_sender_first_awaiting(
        input: (RawLocalEventLake, LocalRawPooledEndpoints),
    ) -> RawLocalEventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::awaiting(sync_raw_lake_awaiting())]
    fn cancel_sync_raw_lake_sender_first_awaiting(
        input: (RawEventLake, SyncRawPooledEndpoints),
    ) -> RawEventLake {
        let (lake, (sender, receiver)) = input;
        drop(sender);
        drop(receiver);
        lake
    }

    #[library_benchmark]
    #[bench::bound(local_raw_lake_bound())]
    fn cancel_local_raw_lake_receiver_first_bound(
        input: (RawLocalEventLake, LocalRawPooledEndpoints),
    ) -> RawLocalEventLake {
        let (lake, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        lake
    }

    #[library_benchmark]
    #[bench::bound(sync_raw_lake_bound())]
    fn cancel_sync_raw_lake_receiver_first_bound(
        input: (RawEventLake, SyncRawPooledEndpoints),
    ) -> RawEventLake {
        let (lake, (sender, receiver)) = input;
        drop(receiver);
        drop(sender);
        lake
    }

    library_benchmark_group!(
        name = rent,
        benchmarks = [rent_local_pooled, rent_sync_pooled]
    );

    library_benchmark_group!(
        name = lifecycle,
        benchmarks = [
            lifecycle_local_boxed,
            lifecycle_sync_boxed,
            lifecycle_local_embedded,
            lifecycle_sync_embedded,
            lifecycle_local_pooled,
            lifecycle_sync_pooled,
            lifecycle_local_raw_pooled,
            lifecycle_sync_raw_pooled,
            lifecycle_local_lake,
            lifecycle_sync_lake,
            lifecycle_local_raw_lake,
            lifecycle_sync_raw_lake,
        ]
    );

    library_benchmark_group!(
        name = lifecycle_await_first,
        benchmarks = [
            lifecycle_await_first_local_pooled,
            lifecycle_await_first_sync_pooled,
        ]
    );

    library_benchmark_group!(
        name = send,
        benchmarks = [
            send_local_bound,
            send_sync_bound,
            send_local_awaiting,
            send_sync_awaiting,
            send_local_disconnected,
            send_sync_disconnected,
        ]
    );

    library_benchmark_group!(
        name = poll,
        benchmarks = [
            poll_local_pending_first,
            poll_sync_pending_first,
            poll_local_pending_repeat,
            poll_sync_pending_repeat,
            poll_local_disconnected,
            poll_sync_disconnected,
        ]
    );

    library_benchmark_group!(
        name = into_value,
        benchmarks = [
            into_value_local_pending,
            into_value_sync_pending,
            into_value_local_ready,
            into_value_sync_ready,
            into_value_local_disconnected,
            into_value_sync_disconnected,
        ]
    );

    library_benchmark_group!(
        name = cancel,
        benchmarks = [
            cancel_local_boxed_sender_first_bound,
            cancel_sync_boxed_sender_first_bound,
            cancel_local_boxed_sender_first_awaiting,
            cancel_sync_boxed_sender_first_awaiting,
            cancel_local_boxed_receiver_first_bound,
            cancel_sync_boxed_receiver_first_bound,
            cancel_local_embedded_sender_first_bound,
            cancel_sync_embedded_sender_first_bound,
            cancel_local_embedded_sender_first_awaiting,
            cancel_sync_embedded_sender_first_awaiting,
            cancel_local_embedded_receiver_first_bound,
            cancel_sync_embedded_receiver_first_bound,
            cancel_local_pooled_sender_first_bound,
            cancel_sync_pooled_sender_first_bound,
            cancel_local_pooled_sender_first_awaiting,
            cancel_sync_pooled_sender_first_awaiting,
            cancel_local_pooled_receiver_first_bound,
            cancel_sync_pooled_receiver_first_bound,
            cancel_local_raw_pooled_sender_first_bound,
            cancel_sync_raw_pooled_sender_first_bound,
            cancel_local_raw_pooled_sender_first_awaiting,
            cancel_sync_raw_pooled_sender_first_awaiting,
            cancel_local_raw_pooled_receiver_first_bound,
            cancel_sync_raw_pooled_receiver_first_bound,
            cancel_local_lake_sender_first_bound,
            cancel_sync_lake_sender_first_bound,
            cancel_local_lake_sender_first_awaiting,
            cancel_sync_lake_sender_first_awaiting,
            cancel_local_lake_receiver_first_bound,
            cancel_sync_lake_receiver_first_bound,
            cancel_local_raw_lake_sender_first_bound,
            cancel_sync_raw_lake_sender_first_bound,
            cancel_local_raw_lake_sender_first_awaiting,
            cancel_sync_raw_lake_sender_first_awaiting,
            cancel_local_raw_lake_receiver_first_bound,
            cancel_sync_raw_lake_receiver_first_bound,
        ]
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{cancel, into_value, lifecycle, lifecycle_await_first, poll, rent, send};

// `--collect-bus=yes` makes Callgrind emit the global bus event (`Ge`), which counts
// lock-prefixed instructions and therefore the atomic read-modify-write operations
// that separate the thread-safe paths from the single-threaded ones. It is an
// instruction count, not a contention or memory-ordering cost. `CallgrindMetrics::
// Default` already reports `Ge` once collection is enabled.
#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes", "--collect-bus=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = rent, lifecycle, lifecycle_await_first, send, poll, into_value, cancel
);
