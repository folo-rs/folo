//! Compares the send/receive performance of `events` with other similar libraries.
//!
//! * `OnceEvent` (in different threading and binding modes)
//! * `oneshot::channel()`
//! * `futures::channel::oneshot::channel()`
//!
//! Each mechanism is exercised in both multi-threaded and single-threaded scenarios.
//!
//! The multi-threaded scenario is simple:
//!
//! 1. There are two threads.
//! 2. Sender thread sends a value to `iter` channels.
//! 3. Receiver thread receives a value from `iter` channels.
//! 4. Measurement ends once both threads have completed their work.
//! 5. Score is average duration of both threads.
//!
//! The single-threaded variant of the above has the sender and receiver
//! on the same thread, exercised in pairs (send + recv + send + recv + ...).
//!
//! The primary scenarios here only cover the send/receive performance and do not
//! measure creation/destruction of the channels (as long as this is not part of the
//! send/receive operation, which it sometimes is).
//!
//! There is a separate `events_overhead_comparison` set of benchmarks for the
//! creation/destruction performance of the different types of channels/events.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use events::{LocalOnceEvent, OnceEvent};
use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type Payload = u128;

static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

fn entrypoint(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(&ProcessorSet::single());

    let mut group = c.benchmark_group("events_comparison_st");

    local_once_event_ref_st(&mut one_thread, &mut group);
    local_once_event_rc_st(&mut one_thread, &mut group);
    local_once_event_ptr_st(&mut one_thread, &mut group);
    oneshot_channel_st(&mut one_thread, &mut group);
    futures_oneshot_channel_st(&mut one_thread, &mut group);

    group.finish();

    let mut two_threads = TWO_PROCESSORS.as_ref().map(ThreadPool::new);

    if let Some(ref mut two_threads) = two_threads {
        let mut group = c.benchmark_group("events_comparison_mt");

        once_event_arc_mt(two_threads, &mut group);
        once_event_ptr_mt(two_threads, &mut group);
        oneshot_channel_mt(two_threads, &mut group);
        futures_oneshot_channel_mt(two_threads, &mut group);

        group.finish();
    }
}

fn once_event_arc_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("once_event_arc", |b| {
        b.iter_custom(|iters| {
            let mut events = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                events.push(Arc::new(OnceEvent::<Payload>::new()));
            }

            let (senders, receivers): (Vec<_>, Vec<_>) =
                events.into_iter().map(|e| e.bind_by_arc()).unzip();

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter_fn(|meta, &()| {
                    if meta.group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter_fn(|(sender, receiver), &()| {
                    if let Some(sender) = sender {
                        sender.send(42);
                    } else if let Some(receiver) = receiver {
                        block_on(receiver).unwrap();
                    }
                })
                .execute_on(pool, iters)
                .mean_duration()
        });
    });
}

fn once_event_ptr_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("once_event_ptr", |b| {
        b.iter_custom(|iters| {
            let mut events = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                events.push(Box::pin(OnceEvent::<Payload>::new()));
            }

            let (senders, receivers): (Vec<_>, Vec<_>) = events
                .iter()
                .map(|e| {
                    // SAFETY: We keep the event alive for the duration of the benchmark,
                    // above in `events`, as required. This is a non-consuming iterator.
                    unsafe { e.as_ref().bind_by_ptr() }
                })
                .unzip();

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter_fn(|meta, &()| {
                    if meta.group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter_fn(|(sender, receiver), &()| {
                    if let Some(sender) = sender {
                        sender.send(42);
                    } else if let Some(receiver) = receiver {
                        block_on(receiver).unwrap();
                    }
                })
                .execute_on(pool, iters)
                .mean_duration()
        });
    });
}

fn oneshot_channel_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("oneshot_channel", |b| {
        b.iter_custom(|iters| {
            let mut senders = Vec::with_capacity(usize::try_from(iters).unwrap());
            let mut receivers = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                let (sender, receiver) = oneshot::channel();
                senders.push(sender);
                receivers.push(receiver);
            }

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter_fn(|meta, &()| {
                    if meta.group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter_fn(|(sender, receiver), &()| {
                    if let Some(sender) = sender {
                        drop(sender.send(42));
                    } else if let Some(receiver) = receiver {
                        block_on(receiver).unwrap();
                    }
                })
                .execute_on(pool, iters)
                .mean_duration()
        });
    });
}

fn futures_oneshot_channel_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("futures_oneshot_channel", |b| {
        b.iter_custom(|iters| {
            let mut senders = Vec::with_capacity(usize::try_from(iters).unwrap());
            let mut receivers = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                #[expect(clippy::absolute_paths, reason = "being explicit")]
                let (sender, receiver) = futures::channel::oneshot::channel();
                senders.push(sender);
                receivers.push(receiver);
            }

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter_fn(|meta, &()| {
                    if meta.group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter_fn(|(sender, receiver), &()| {
                    if let Some(sender) = sender {
                        _ = sender.send(42);
                    } else if let Some(receiver) = receiver {
                        block_on(receiver).unwrap();
                    }
                })
                .execute_on(pool, iters)
                .mean_duration()
        });
    });
}

fn local_once_event_ref_st(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    Run::new()
        .iter_fn(|(), &()| {
            // This binding mode is really awkward to use but that's the deal with Rust references.
            let event = LocalOnceEvent::<Payload>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "local_once_event_ref");
}

fn local_once_event_rc_st(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    Run::new()
        .prepare_iter_fn(|_, &()| {
            let event = Rc::new(LocalOnceEvent::<Payload>::new());
            event.bind_by_rc()
        })
        .iter_fn(|(sender, receiver), &()| {
            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "local_once_event_rc");
}

fn local_once_event_ptr_st(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    Run::new()
        .prepare_iter_fn(|_, &()| {
            let event = Box::pin(LocalOnceEvent::<Payload>::new());

            // SAFETY: We keep the event alive for the duration of the iteration,
            // dropping it only at the moment of cleanup of each iteration.
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            (event, sender, receiver)
        })
        .iter_fn(|(event, sender, receiver), &()| {
            sender.send(42);
            block_on(receiver).unwrap();

            // Event is destroyed in cleanup stage.
            event
        })
        .execute_criterion_on(pool, group, "local_once_event_ptr");
}

fn oneshot_channel_st(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    Run::new()
        .prepare_iter_fn(|_, &()| oneshot::channel())
        .iter_fn(|(sender, receiver), &()| {
            drop(sender.send(42));
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "oneshot_channel");
}

fn futures_oneshot_channel_st(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    #[expect(clippy::absolute_paths, reason = "being explicit")]
    Run::new()
        .prepare_iter_fn(|_, &()| futures::channel::oneshot::channel())
        .iter_fn(|(sender, receiver), &()| {
            _ = sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "futures_oneshot_channel");
}
