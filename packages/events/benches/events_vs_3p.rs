//! Benchmark comparing event performance of the `events` package
//! against third-party alternatives (oneshot, `futures::oneshot`).
//!
//! The benchmark simulates a high-churn scenario:
//! 1. Pre-fill with many events (10,000).
//! 2. Repeatedly add batches of new events and complete batches of existing events.
#![allow(
    dead_code,
    clippy::collection_is_never_read,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::explicit_counter_loop,
    clippy::integer_division,
    clippy::indexing_slicing,
    missing_docs,
    reason = "duty of care is slightly lowered for benchmark code"
)]

use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};
use events::*;
use futures::channel::oneshot as futures_oneshot;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Number of events to pre-fill before starting the timed benchmark span.
/// This ensures we start from a "hot state" with existing events.
const INITIAL_ITEMS: u64 = 10_000;

/// Number of events to add and complete in each batch operation.
const BATCH_SIZE: u64 = 15;

/// Number of batch operations to perform during the timed span.
/// Each batch adds `BATCH_SIZE` events and completes `BATCH_SIZE` existing events.
const BATCH_COUNT: u64 = 10_000;

/// Payload type for all events.
type Payload = u128;

fn churn_event_benchmark(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("events_vs_3p");

    // Criterion's default 5 seconds does not give the precision we need.
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(1000);

    // LocalOnceEvent standalone, by rc binding
    let allocs_op = allocs.operation("LocalOnceEvent rc");
    group.bench_function("LocalOnceEvent rc", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);
            let mut all_senders = Vec::with_capacity(iters as usize);
            let mut all_receivers = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::<Rc<LocalOnceEvent<Payload>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
                all_senders.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
                all_receivers.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items and bind them outside timed span
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                for _ in 0..INITIAL_ITEMS {
                    let event = Rc::new(LocalOnceEvent::new());
                    let (sender, receiver) = event.bind_by_rc();
                    events.push(event);
                    senders.push(sender);
                    receivers.push(receiver);
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let event = Rc::new(LocalOnceEvent::new());
                        let (sender, receiver) = event.bind_by_rc();
                        events.push(event);
                        senders.push(sender);
                        receivers.push(receiver);
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..senders.len());
                        let sender = senders.swap_remove(target_index);
                        let receiver = receivers.swap_remove(target_index);
                        events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalOnceEvent standalone, by ptr binding
    let allocs_op = allocs.operation("LocalOnceEvent ptr");
    group.bench_function("LocalOnceEvent ptr", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);
            let mut all_senders = Vec::with_capacity(iters as usize);
            let mut all_receivers = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::<Pin<Box<LocalOnceEvent<Payload>>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
                all_senders.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
                all_receivers.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items and bind them outside timed span
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                for _ in 0..INITIAL_ITEMS {
                    let event = Box::pin(LocalOnceEvent::new());
                    // SAFETY: Event is pinned and outlives the sender/receiver
                    let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
                    events.push(event);
                    senders.push(sender);
                    receivers.push(receiver);
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let event = Box::pin(LocalOnceEvent::new());
                        // SAFETY: Event is pinned and outlives the sender/receiver
                        let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
                        events.push(event);
                        senders.push(sender);
                        receivers.push(receiver);
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..senders.len());
                        let sender = senders.swap_remove(target_index);
                        let receiver = receivers.swap_remove(target_index);
                        events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalOnceEvent standalone, in-place initialized by ptr binding
    let allocs_op = allocs.operation("LocalOnceEvent in-place ptr");
    group.bench_function("LocalOnceEvent in-place ptr", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);
            let mut all_senders = Vec::with_capacity(iters as usize);
            let mut all_receivers = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::<
                    Pin<Box<std::mem::MaybeUninit<LocalOnceEvent<Payload>>>>,
                >::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize
                ));
                all_senders.push(
                    Vec::<LocalOnceSender<PtrLocalEvent<Payload>>>::with_capacity(
                        (INITIAL_ITEMS + BATCH_SIZE) as usize,
                    ),
                );
                all_receivers.push(
                    Vec::<LocalOnceReceiver<PtrLocalEvent<Payload>>>::with_capacity(
                        (INITIAL_ITEMS + BATCH_SIZE) as usize,
                    ),
                );
            }

            // Pre-fill with initial items and bind them outside timed span
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                for _ in 0..INITIAL_ITEMS {
                    let mut event_storage = Box::pin(std::mem::MaybeUninit::uninit());
                    // SAFETY: We manage the storage correctly and drop events after use
                    let (sender, receiver) =
                        unsafe { LocalOnceEvent::new_in_place_by_ptr(event_storage.as_mut()) };
                    events.push(event_storage);
                    senders.push(sender);
                    receivers.push(receiver);
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let mut event_storage = Box::pin(std::mem::MaybeUninit::uninit());
                        // SAFETY: We manage the storage correctly and drop events after use
                        let (sender, receiver) =
                            unsafe { LocalOnceEvent::new_in_place_by_ptr(event_storage.as_mut()) };
                        events.push(event_storage);
                        senders.push(sender);
                        receivers.push(receiver);
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..senders.len());
                        let sender = senders.swap_remove(target_index);
                        let receiver = receivers.swap_remove(target_index);
                        events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalOnceEvent standalone, managed binding
    let allocs_op = allocs.operation("LocalOnceEvent managed");
    group.bench_function("LocalOnceEvent managed", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::<(
                    LocalOnceSender<ManagedLocalEvent<Payload>>,
                    LocalOnceReceiver<ManagedLocalEvent<Payload>>,
                )>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize
                ));
            }

            // Pre-fill with initial items outside timed span
            for events in &mut all_events {
                for _ in 0..INITIAL_ITEMS {
                    events.push(LocalOnceEvent::new_managed());
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for events in &mut all_events {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        events.push(LocalOnceEvent::new_managed());
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // OnceEvent standalone, arc binding
    let allocs_op = allocs.operation("OnceEvent arc");
    group.bench_function("OnceEvent arc", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);
            let mut all_senders = Vec::with_capacity(iters as usize);
            let mut all_receivers = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::<Arc<OnceEvent<Payload>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
                all_senders.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
                all_receivers.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items and bind them outside timed span
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                for _ in 0..INITIAL_ITEMS {
                    let event = Arc::new(OnceEvent::new());
                    let (sender, receiver) = event.bind_by_arc();
                    events.push(event);
                    senders.push(sender);
                    receivers.push(receiver);
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let event = Arc::new(OnceEvent::new());
                        let (sender, receiver) = event.bind_by_arc();
                        events.push(event);
                        senders.push(sender);
                        receivers.push(receiver);
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..senders.len());
                        let sender = senders.swap_remove(target_index);
                        let receiver = receivers.swap_remove(target_index);
                        events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // OnceEvent standalone, ptr binding
    let allocs_op = allocs.operation("OnceEvent ptr");
    group.bench_function("OnceEvent ptr", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);
            let mut all_senders = Vec::with_capacity(iters as usize);
            let mut all_receivers = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::<Pin<Box<OnceEvent<Payload>>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
                all_senders.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
                all_receivers.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items and bind them outside timed span
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                for _ in 0..INITIAL_ITEMS {
                    let event = Box::pin(OnceEvent::new());
                    // SAFETY: Event is pinned and outlives the sender/receiver
                    let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
                    events.push(event);
                    senders.push(sender);
                    receivers.push(receiver);
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let event = Box::pin(OnceEvent::new());
                        // SAFETY: Event is pinned and outlives the sender/receiver
                        let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
                        events.push(event);
                        senders.push(sender);
                        receivers.push(receiver);
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..senders.len());
                        let sender = senders.swap_remove(target_index);
                        let receiver = receivers.swap_remove(target_index);
                        events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // OnceEvent standalone, in-place by ptr binding
    let allocs_op = allocs.operation("OnceEvent in-place ptr");
    group.bench_function("OnceEvent in-place ptr", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);
            let mut all_senders = Vec::with_capacity(iters as usize);
            let mut all_receivers = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(
                    Vec::<Pin<Box<std::mem::MaybeUninit<OnceEvent<Payload>>>>>::with_capacity(
                        (INITIAL_ITEMS + BATCH_SIZE) as usize,
                    ),
                );
                all_senders.push(Vec::<OnceSender<PtrEvent<Payload>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
                all_receivers.push(Vec::<OnceReceiver<PtrEvent<Payload>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
            }

            // Pre-fill with initial items and bind them outside timed span
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                for _ in 0..INITIAL_ITEMS {
                    let mut event_storage = Box::pin(std::mem::MaybeUninit::uninit());
                    // SAFETY: We manage the storage correctly and drop events after use
                    let (sender, receiver) =
                        unsafe { OnceEvent::new_in_place_by_ptr(event_storage.as_mut()) };
                    events.push(event_storage);
                    senders.push(sender);
                    receivers.push(receiver);
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for ((events, senders), receivers) in all_events
                .iter_mut()
                .zip(all_senders.iter_mut())
                .zip(all_receivers.iter_mut())
            {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let mut event_storage = Box::pin(std::mem::MaybeUninit::uninit());
                        // SAFETY: We manage the storage correctly and drop events after use
                        let (sender, receiver) =
                            unsafe { OnceEvent::new_in_place_by_ptr(event_storage.as_mut()) };
                        events.push(event_storage);
                        senders.push(sender);
                        receivers.push(receiver);
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..senders.len());
                        let sender = senders.swap_remove(target_index);
                        let receiver = receivers.swap_remove(target_index);
                        events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // OnceEvent standalone, managed binding
    let allocs_op = allocs.operation("OnceEvent managed");
    group.bench_function("OnceEvent managed", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::<(
                    OnceSender<ManagedEvent<Payload>>,
                    OnceReceiver<ManagedEvent<Payload>>,
                )>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize
                ));
            }

            // Pre-fill with initial items outside timed span
            for events in &mut all_events {
                for _ in 0..INITIAL_ITEMS {
                    events.push(OnceEvent::new_managed());
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for events in &mut all_events {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        events.push(OnceEvent::new_managed());
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalOnceEventPool, by rc binding
    let allocs_op = allocs.operation("LocalOnceEventPool rc");
    group.bench_function("LocalOnceEventPool rc", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and event vectors outside timed span
            for _ in 0..iters {
                all_pools.push(Rc::new(LocalOnceEventPool::<Payload>::new()));
                all_events.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, events) in all_pools.iter().zip(all_events.iter_mut()) {
                for _ in 0..INITIAL_ITEMS {
                    let (sender, receiver) = pool.bind_by_rc();
                    events.push((sender, receiver));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, events) in all_pools.iter().zip(all_events.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let (sender, receiver) = pool.bind_by_rc();
                        events.push((sender, receiver));
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalOnceEventPool, by ptr binding
    let allocs_op = allocs.operation("LocalOnceEventPool ptr");
    group.bench_function("LocalOnceEventPool ptr", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and event vectors outside timed span
            for _ in 0..iters {
                all_pools.push(Box::pin(LocalOnceEventPool::<Payload>::new()));
                all_events.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, events) in all_pools.iter().zip(all_events.iter_mut()) {
                for _ in 0..INITIAL_ITEMS {
                    // SAFETY: Pool is pinned and outlives the sender/receiver
                    let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };
                    events.push((sender, receiver));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, events) in all_pools.iter().zip(all_events.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        // SAFETY: Pool is pinned and outlives the sender/receiver
                        let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };
                        events.push((sender, receiver));
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // OnceEventPool, by arc binding
    let allocs_op = allocs.operation("OnceEventPool arc");
    group.bench_function("OnceEventPool arc", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and event vectors outside timed span
            for _ in 0..iters {
                all_pools.push(Arc::new(OnceEventPool::<Payload>::new()));
                all_events.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, events) in all_pools.iter_mut().zip(all_events.iter_mut()) {
                for _ in 0..INITIAL_ITEMS {
                    let (sender, receiver) = pool.bind_by_arc();
                    events.push((sender, receiver));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, events) in all_pools.iter_mut().zip(all_events.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let (sender, receiver) = pool.bind_by_arc();
                        events.push((sender, receiver));
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // OnceEventPool, by ptr binding
    let allocs_op = allocs.operation("OnceEventPool ptr");
    group.bench_function("OnceEventPool ptr", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and event vectors outside timed span
            for _ in 0..iters {
                all_pools.push(Box::pin(OnceEventPool::<Payload>::new()));
                all_events.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, events) in all_pools.iter().zip(all_events.iter_mut()) {
                for _ in 0..INITIAL_ITEMS {
                    // SAFETY: Pool is pinned and outlives the sender/receiver
                    let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };
                    events.push((sender, receiver));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, events) in all_pools.iter().zip(all_events.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        // SAFETY: Pool is pinned and outlives the sender/receiver
                        let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };
                        events.push((sender, receiver));
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        sender.send(next_value);
                        let _value = receiver.into_value().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // oneshot::channel
    let allocs_op = allocs.operation("oneshot::channel");
    group.bench_function("oneshot::channel", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for events in &mut all_events {
                for _ in 0..INITIAL_ITEMS {
                    let (sender, receiver) = oneshot::channel::<Payload>();
                    events.push((sender, receiver));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for events in &mut all_events {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let (sender, receiver) = oneshot::channel::<Payload>();
                        events.push((sender, receiver));
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        drop(sender.send(next_value));
                        let _value = receiver.recv().unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    // futures::channel::oneshot::channel
    let allocs_op = allocs.operation("futures::oneshot::channel");
    group.bench_function("futures::oneshot::channel", |b| {
        b.iter_custom(|iters| {
            let mut all_events = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_events.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for events in &mut all_events {
                for _ in 0..INITIAL_ITEMS {
                    let (sender, receiver) = futures_oneshot::channel::<Payload>();
                    events.push((sender, receiver));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for events in &mut all_events {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = u128::from(INITIAL_ITEMS);

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new events
                    for _ in 0..BATCH_SIZE {
                        let (sender, receiver) = futures_oneshot::channel::<Payload>();
                        events.push((sender, receiver));
                    }

                    // Complete BATCH_SIZE random existing events
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..events.len());
                        let (sender, receiver) = events.swap_remove(target_index);

                        // Complete the event lifecycle
                        let _unused = sender.send(next_value);
                        let _value = futures::executor::block_on(receiver).unwrap();

                        next_value = next_value.wrapping_add(1);
                    }
                }
            }
            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}

criterion_group!(benches, churn_event_benchmark);
criterion_main!(benches);
