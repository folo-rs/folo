//! Compares the performance of `events` with other similar libraries when processing 10k events.
//!
//! * `OnceEvent` (in different threading and binding modes)
//! * `oneshot::channel()`
//! * `futures::channel::oneshot::channel()`
//!
//! While similar to `events_full_cycle_comparison`, this explores performance from a slightly
//! different perspective. Each iteration is 10 000 events, resolved in random order. Does the
//! pre-creation and randomized order cause differences between bindings? Or with competitors?

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
#![allow(
    clippy::drop_non_drop,
    reason = "Explicit drops for clarity in benchmarks"
)]
#![allow(
    clippy::large_stack_frames,
    reason = "Some scenarios specifically test stack allocation"
)]

use std::hint::black_box;
use std::iter;
use std::mem::MaybeUninit;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;

use alloc_tracker::{Allocator, Session};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use events::{LocalOnceEvent, LocalOnceEventPool, OnceEvent, OnceEventPool};
use many_cpus::ProcessorSet;
use par_bench::{ResourceUsageExt, Run, ThreadPool};
use rand::seq::SliceRandom;
use spin_on::spin_on;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type Payload = u128;

#[cfg(not(debug_assertions))]
const EVENT_COUNT: usize = 10_000;

// The events are much bigger if debug assertions are enabled, so just use a few of them.
// This is generally used only for testing the benchmarks.
#[cfg(debug_assertions)]
const EVENT_COUNT: usize = 10;

fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();
    let mut one_thread = ThreadPool::new(ProcessorSet::single());

    let mut group = c.benchmark_group("events_10k");

    local_once_event_ref(&mut one_thread, &mut group, &allocs);
    local_once_event_rc(&mut one_thread, &mut group, &allocs);
    local_once_event_ptr(&mut one_thread, &mut group, &allocs);
    local_once_event_in_place_ptr(&mut one_thread, &mut group, &allocs);
    local_once_event_managed(&mut one_thread, &mut group, &allocs);
    once_event_arc(&mut one_thread, &mut group, &allocs);
    once_event_ptr(&mut one_thread, &mut group, &allocs);
    once_event_in_place_ptr(&mut one_thread, &mut group, &allocs);
    once_event_managed(&mut one_thread, &mut group, &allocs);
    pooled_local_once_event_ref(&mut one_thread, &mut group, &allocs);
    pooled_local_once_event_rc(&mut one_thread, &mut group, &allocs);
    pooled_local_once_event_ptr(&mut one_thread, &mut group, &allocs);
    pooled_once_event_ref(&mut one_thread, &mut group, &allocs);
    pooled_once_event_arc(&mut one_thread, &mut group, &allocs);
    pooled_once_event_ptr(&mut one_thread, &mut group, &allocs);
    oneshot_channel(&mut one_thread, &mut group, &allocs);
    futures_oneshot_channel(&mut one_thread, &mut group, &allocs);

    group.finish();

    allocs.print_to_stdout();
}

fn local_once_event_ref(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("local_once_event_ref", |measure| measure.allocs(allocs))
        .iter(|_| {
            let events = iter::repeat_with(LocalOnceEvent::<Payload>::new)
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            let mut senders_and_receivers = events
                .iter()
                .map(LocalOnceEvent::bind_by_ref)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "local_once_event_ref");
}

fn local_once_event_rc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("local_once_event_rc", |measure| measure.allocs(allocs))
        .iter(|_| {
            let mut senders_and_receivers = iter::repeat_with(|| {
                let event = Rc::new(LocalOnceEvent::<Payload>::new());
                event.bind_by_rc()
            })
            .take(EVENT_COUNT)
            .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "local_once_event_rc");
}

fn local_once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("local_once_event_ptr", |measure| measure.allocs(allocs))
        .iter(|_| {
            // Use pinned local storage to avoid heap allocations.
            let events: [MaybeUninit<LocalOnceEvent<Payload>>; EVENT_COUNT] =
                std::array::from_fn(|_| MaybeUninit::uninit());
            let mut events = pin!(events);

            let mut senders_and_receivers = Vec::with_capacity(EVENT_COUNT);

            // Initialize all events and bind them.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We keep the events alive for the duration of this scope and
                // the pinned array ensures the memory locations are stable.
                let pin_ref = unsafe { Pin::new_unchecked(event_storage) };
                // SAFETY: The pinned storage is valid and stable for the event lifetime.
                let (sender, receiver) =
                    unsafe { LocalOnceEvent::<Payload>::new_in_place_by_ptr(pin_ref) };
                senders_and_receivers.push((sender, receiver));
            }

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }

            // SAFETY: We initialized all storage above and have dropped both senders and receivers.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We initialized this storage above and dropped all references.
                unsafe {
                    event_storage.assume_init_drop();
                }
            }
        })
        .execute_criterion_on(pool, group, "local_once_event_ptr");
}

fn local_once_event_in_place_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("local_once_event_in_place_ptr", |measure| {
            measure.allocs(allocs)
        })
        .iter(|_| {
            // Use pinned local storage to avoid heap allocations.
            let events: [MaybeUninit<LocalOnceEvent<Payload>>; EVENT_COUNT] =
                std::array::from_fn(|_| MaybeUninit::uninit());
            let mut events = pin!(events);

            let mut senders_and_receivers = Vec::with_capacity(EVENT_COUNT);

            // Initialize all events and bind them.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We keep the events alive for the duration of this scope and
                // the pinned array ensures the memory locations are stable.
                let pin_ref = unsafe { Pin::new_unchecked(event_storage) };
                // SAFETY: The pinned storage is valid and stable for the event lifetime.
                let (sender, receiver) =
                    unsafe { LocalOnceEvent::<Payload>::new_in_place_by_ptr(pin_ref) };
                senders_and_receivers.push((sender, receiver));
            }

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }

            // SAFETY: We initialized all storage above and have dropped both senders and receivers.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We initialized this storage above and dropped all references.
                unsafe {
                    event_storage.assume_init_drop();
                }
            }
        })
        .execute_criterion_on(pool, group, "local_once_event_in_place_ptr");
}

fn local_once_event_managed(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("local_once_event_managed", |measure| measure.allocs(allocs))
        .iter(|_| {
            let mut senders_and_receivers =
                iter::repeat_with(LocalOnceEvent::<Payload>::new_managed)
                    .take(EVENT_COUNT)
                    .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "local_once_event_managed");
}

fn once_event_arc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("once_event_arc", |measure| measure.allocs(allocs))
        .iter(|_| {
            let mut senders_and_receivers = iter::repeat_with(|| {
                let event = Arc::new(OnceEvent::<Payload>::new());
                event.bind_by_arc()
            })
            .take(EVENT_COUNT)
            .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "once_event_arc");
}

fn once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("once_event_ptr", |measure| measure.allocs(allocs))
        .iter(|_| {
            // Use pinned local storage to avoid heap allocations.
            let events: [MaybeUninit<OnceEvent<Payload>>; EVENT_COUNT] =
                std::array::from_fn(|_| MaybeUninit::uninit());
            let mut events = pin!(events);

            let mut senders_and_receivers = Vec::with_capacity(EVENT_COUNT);

            // Initialize all events and bind them.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We keep the events alive for the duration of this scope and
                // the pinned array ensures the memory locations are stable.
                let pin_ref = unsafe { Pin::new_unchecked(event_storage) };
                // SAFETY: The pinned storage is valid and stable for the event lifetime.
                let (sender, receiver) =
                    unsafe { OnceEvent::<Payload>::new_in_place_by_ptr(pin_ref) };
                senders_and_receivers.push((sender, receiver));
            }

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }

            // SAFETY: We initialized all storage above and have dropped both senders and receivers.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We initialized this storage above and dropped all references.
                unsafe {
                    event_storage.assume_init_drop();
                }
            }
        })
        .execute_criterion_on(pool, group, "once_event_ptr");
}

fn once_event_in_place_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("once_event_in_place_ptr", |measure| measure.allocs(allocs))
        .iter(|_| {
            // Use pinned local storage to avoid heap allocations.
            let events: [MaybeUninit<OnceEvent<Payload>>; EVENT_COUNT] =
                std::array::from_fn(|_| MaybeUninit::uninit());
            let mut events = pin!(events);

            let mut senders_and_receivers = Vec::with_capacity(EVENT_COUNT);

            // Initialize all events and bind them.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We keep the events alive for the duration of this scope and
                // the pinned array ensures the memory locations are stable.
                let pin_ref = unsafe { Pin::new_unchecked(event_storage) };
                // SAFETY: The pinned storage is valid and stable for the event lifetime.
                let (sender, receiver) =
                    unsafe { OnceEvent::<Payload>::new_in_place_by_ptr(pin_ref) };
                senders_and_receivers.push((sender, receiver));
            }

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }

            // SAFETY: We initialized all storage above and have dropped both senders and receivers.
            for i in 0..EVENT_COUNT {
                // SAFETY: i is within bounds of the array.
                let array_ref = unsafe { events.as_mut().get_unchecked_mut() };
                // SAFETY: i is within bounds of the array.
                let event_storage = unsafe { array_ref.get_unchecked_mut(i) };
                // SAFETY: We initialized this storage above and dropped all references.
                unsafe {
                    event_storage.assume_init_drop();
                }
            }
        })
        .execute_criterion_on(pool, group, "once_event_in_place_ptr");
}

fn once_event_managed(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("once_event_managed", |measure| measure.allocs(allocs))
        .iter(|_| {
            let mut senders_and_receivers = iter::repeat_with(OnceEvent::<Payload>::new_managed)
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "once_event_managed");
}

fn oneshot_channel(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("oneshot_channel", |measure| measure.allocs(allocs))
        .iter(|_| {
            let mut senders_and_receivers = iter::repeat_with(oneshot::channel)
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                drop(sender.send(42));
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "oneshot_channel");
}

fn futures_oneshot_channel(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    #[expect(clippy::absolute_paths, reason = "being explicit")]
    Run::new()
        .measure_resource_usage("futures_oneshot_channel", |measure| measure.allocs(allocs))
        .iter(|_| {
            let mut senders_and_receivers = iter::repeat_with(futures::channel::oneshot::channel)
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                _ = sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "futures_oneshot_channel");
}

fn pooled_local_once_event_ref(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("pooled_local_once_event_ref", |measure| {
            measure.allocs(allocs)
        })
        .iter(|_| {
            let event_pool = LocalOnceEventPool::<Payload>::new();
            let mut senders_and_receivers = iter::repeat_with(|| event_pool.bind_by_ref())
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_ref");
}

fn pooled_local_once_event_rc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("pooled_local_once_event_rc", |measure| {
            measure.allocs(allocs)
        })
        .iter(|_| {
            let event_pool = Rc::new(LocalOnceEventPool::<Payload>::new());
            let mut senders_and_receivers = iter::repeat_with(|| event_pool.bind_by_rc())
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_rc");
}

fn pooled_local_once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("pooled_local_once_event_ptr", |measure| {
            measure.allocs(allocs)
        })
        .iter(|_| {
            let event_pool = Box::pin(LocalOnceEventPool::<Payload>::new());
            let mut senders_and_receivers = iter::repeat_with(|| {
                // SAFETY: We keep the pool alive for the duration of this scope.
                unsafe { event_pool.as_ref().bind_by_ptr() }
            })
            .take(EVENT_COUNT)
            .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_ptr");
}

fn pooled_once_event_ref(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("pooled_once_event_ref", |measure| measure.allocs(allocs))
        .iter(|_| {
            let event_pool = OnceEventPool::<Payload>::new();
            let mut senders_and_receivers = iter::repeat_with(|| event_pool.bind_by_ref())
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "pooled_once_event_ref");
}

fn pooled_once_event_arc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("pooled_once_event_arc", |measure| measure.allocs(allocs))
        .iter(|_| {
            let event_pool = Arc::new(OnceEventPool::<Payload>::new());
            let mut senders_and_receivers = iter::repeat_with(|| event_pool.bind_by_arc())
                .take(EVENT_COUNT)
                .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "pooled_once_event_arc");
}

fn pooled_once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
) {
    Run::new()
        .measure_resource_usage("pooled_once_event_ptr", |measure| measure.allocs(allocs))
        .iter(|_| {
            let event_pool = Box::pin(OnceEventPool::<Payload>::new());
            let mut senders_and_receivers = iter::repeat_with(|| {
                // SAFETY: We keep the pool alive for the duration of this scope.
                unsafe { event_pool.as_ref().bind_by_ptr() }
            })
            .take(EVENT_COUNT)
            .collect::<Vec<_>>();

            senders_and_receivers.shuffle(&mut rand::rng());

            for (sender, receiver) in senders_and_receivers {
                sender.send(42);
                black_box(spin_on(receiver).unwrap());
            }
        })
        .execute_criterion_on(pool, group, "pooled_once_event_ptr");
}
