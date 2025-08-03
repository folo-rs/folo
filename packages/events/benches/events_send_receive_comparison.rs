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

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use events::{LocalOnceEvent, LocalOnceEventPool, OnceEvent, OnceEventPool};
use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{ResourceUsageExt, Run, ThreadPool};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type Payload = u128;

static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();
    let processor_time = TimeSession::new();
    let mut one_thread = ThreadPool::new(ProcessorSet::single());

    let mut group = c.benchmark_group("events_comparison_st");

    local_once_event_ref_st(&mut one_thread, &mut group, &allocs, &processor_time);
    local_once_event_rc_st(&mut one_thread, &mut group, &allocs, &processor_time);
    local_once_event_ptr_st(&mut one_thread, &mut group, &allocs, &processor_time);
    once_event_arc_st(&mut one_thread, &mut group, &allocs, &processor_time);
    once_event_ptr_st(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_local_once_event_ref_st(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_local_once_event_rc_st(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_local_once_event_ptr_st(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_once_event_ref_st(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_once_event_arc_st(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_once_event_ptr_st(&mut one_thread, &mut group, &allocs, &processor_time);
    oneshot_channel_st(&mut one_thread, &mut group, &allocs, &processor_time);
    futures_oneshot_channel_st(&mut one_thread, &mut group, &allocs, &processor_time);

    group.finish();

    let mut two_threads = TWO_PROCESSORS.as_ref().map(ThreadPool::new);

    if let Some(ref mut two_threads) = two_threads {
        let mut group = c.benchmark_group("events_comparison_mt");

        once_event_arc_mt(two_threads, &mut group);
        once_event_ptr_mt(two_threads, &mut group);
        pooled_once_event_ref_mt(two_threads, &mut group);
        pooled_once_event_arc_mt(two_threads, &mut group);
        pooled_once_event_ptr_mt(two_threads, &mut group);
        oneshot_channel_mt(two_threads, &mut group);
        futures_oneshot_channel_mt(two_threads, &mut group);

        group.finish();
    }

    allocs.print_to_stdout();
    processor_time.print_to_stdout();
}

fn once_event_arc_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("once_event_arc", |b| {
        b.iter_custom(|iters| {
            let mut events = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                events.push(Arc::new(OnceEvent::<Payload>::new()));
            }

            // Last of the sender/receiver will drop the event. This matches `oneshot` behavior.
            let (senders, receivers): (Vec<_>, Vec<_>) =
                events.into_iter().map(|e| e.bind_by_arc()).unzip();

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter(|args| {
                    if args.meta().group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter(|mut args| {
                    let (sender, receiver) = args.take_iter_state();

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
            // We keep the events alive until the end of the run. This is not exactly fair to the
            // competition as it means we postpone the cleanup but as the event is shared via raw
            // pointers between sender and receiver, we cannot know when to drop it during the loop.
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
                .prepare_iter(|args| {
                    if args.meta().group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter(|mut args| {
                    let (sender, receiver) = args.take_iter_state();

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
                .prepare_iter(|args| {
                    if args.meta().group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter(|mut args| {
                    let (sender, receiver) = args.take_iter_state();

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
                .prepare_iter(|args| {
                    if args.meta().group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter(|mut args| {
                    let (sender, receiver) = args.take_iter_state();

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

fn pooled_once_event_ref_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("pooled_once_event_ref", |b| {
        b.iter_custom(|iters| {
            let event_pool = OnceEventPool::<Payload>::new();

            let mut senders = Vec::with_capacity(usize::try_from(iters).unwrap());
            let mut receivers = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                let (sender, receiver) = event_pool.bind_by_ref();
                senders.push(sender);
                receivers.push(receiver);
            }

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter(|args| {
                    if args.meta().group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter(|mut args| {
                    let (sender, receiver) = args.take_iter_state();

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

fn pooled_once_event_arc_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("pooled_once_event_arc", |b| {
        b.iter_custom(|iters| {
            let event_pool = Arc::new(OnceEventPool::<Payload>::new());

            let mut senders = Vec::with_capacity(usize::try_from(iters).unwrap());
            let mut receivers = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                let (sender, receiver) = event_pool.bind_by_arc();
                senders.push(sender);
                receivers.push(receiver);
            }

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter(|args| {
                    if args.meta().group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter(|mut args| {
                    let (sender, receiver) = args.take_iter_state();

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

fn pooled_once_event_ptr_mt(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("pooled_once_event_ptr", |b| {
        b.iter_custom(|iters| {
            // We keep the pool alive until the end of the run.
            let event_pool = Box::pin(OnceEventPool::<Payload>::new());

            let mut senders = Vec::with_capacity(usize::try_from(iters).unwrap());
            let mut receivers = Vec::with_capacity(usize::try_from(iters).unwrap());

            for _ in 0..iters {
                // SAFETY: We keep the pool alive for the duration of the benchmark,
                // above in `event_pool`, as required.
                let (sender, receiver) = unsafe { event_pool.as_ref().bind_by_ptr() };
                senders.push(sender);
                receivers.push(receiver);
            }

            let senders = Mutex::new(senders);
            let receivers = Mutex::new(receivers);

            Run::new()
                // Group 0 sends, group 1 receives.
                .groups(nz!(2))
                .prepare_iter(|args| {
                    if args.meta().group_index() == 0 {
                        (Some(senders.lock().unwrap().pop().unwrap()), None)
                    } else {
                        (None, Some(receivers.lock().unwrap().pop().unwrap()))
                    }
                })
                .iter(|mut args| {
                    let (sender, receiver) = args.take_iter_state();

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

fn local_once_event_ref_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_iter(|_| LocalOnceEvent::<Payload>::new())
        .measure_resource_usage("local_once_event_ref", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|args| {
            // We bind a bit later here than in the other benchmarks because the by-ref mode
            // is a bit cumbersome. Good enough, just remember there is some tiny overhead.
            let (sender, receiver) = args.iter_state().bind_by_ref();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "local_once_event_ref");
}

fn local_once_event_rc_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_iter(|_| {
            let event = Rc::new(LocalOnceEvent::<Payload>::new());
            event.bind_by_rc()
        })
        .measure_resource_usage("local_once_event_rc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "local_once_event_rc");
}

fn local_once_event_ptr_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_iter(|_| {
            let event = Box::pin(LocalOnceEvent::<Payload>::new());

            // SAFETY: We keep the event alive for the duration of the iteration,
            // dropping it only at the moment of cleanup of each iteration.
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            (event, sender, receiver)
        })
        .measure_resource_usage("local_once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (event, sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();

            // We are required to keep the event alive until both sender and receiver are gone.
            // We drop it as part of the measured part to match the behavior of `oneshot`.
            drop(event);
        })
        .execute_criterion_on(pool, group, "local_once_event_ptr");
}

fn once_event_arc_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_iter(|_| {
            let event = Arc::new(OnceEvent::<Payload>::new());
            event.bind_by_arc()
        })
        .measure_resource_usage("once_event_arc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "once_event_arc");
}

fn once_event_ptr_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_iter(|_| {
            let event = Box::pin(OnceEvent::<Payload>::new());

            // SAFETY: We keep the event alive for the duration of the iteration,
            // dropping it only at the moment of cleanup of each iteration.
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            (event, sender, receiver)
        })
        .measure_resource_usage("once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (event, sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();

            // We are required to keep the event alive until both sender and receiver are gone.
            // We drop it as part of the measured part to match the behavior of `oneshot`.
            drop(event);
        })
        .execute_criterion_on(pool, group, "once_event_ptr");
}

fn oneshot_channel_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_iter(|_| oneshot::channel())
        .measure_resource_usage("oneshot_channel", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            drop(sender.send(42));
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "oneshot_channel");
}

fn futures_oneshot_channel_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    #[expect(clippy::absolute_paths, reason = "being explicit")]
    Run::new()
        .prepare_iter(|_| futures::channel::oneshot::channel())
        .measure_resource_usage("futures_oneshot_channel", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            _ = sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "futures_oneshot_channel");
}

fn pooled_local_once_event_ref_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| LocalOnceEventPool::<Payload>::new())
        .measure_resource_usage("pooled_local_once_event_ref", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|args| {
            // We bind a bit later here than in the other benchmarks because the by-ref mode
            // is a bit cumbersome. Good enough, just remember there is some tiny overhead.
            let (sender, receiver) = args.thread_state().bind_by_ref();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_ref");
}

fn pooled_local_once_event_rc_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Rc::new(LocalOnceEventPool::<Payload>::new()))
        .prepare_iter(|args| args.thread_state().bind_by_rc())
        .measure_resource_usage("pooled_local_once_event_rc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_rc");
}

fn pooled_local_once_event_ptr_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Box::pin(LocalOnceEventPool::<Payload>::new()))
        .prepare_iter(|args| {
            // SAFETY: We keep the pool alive for the duration of the iteration,
            // dropping it only at the moment of cleanup of each iteration.
            let (sender, receiver) = unsafe { args.thread_state().as_ref().bind_by_ptr() };

            (sender, receiver)
        })
        .measure_resource_usage("pooled_local_once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_ptr");
}

fn pooled_once_event_ref_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| OnceEventPool::<Payload>::new())
        .measure_resource_usage("pooled_once_event_ref", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|args| {
            // We bind a bit later here than in the other benchmarks because the by-ref mode
            // is a bit cumbersome. Good enough, just remember there is some tiny overhead.
            let (sender, receiver) = args.thread_state().bind_by_ref();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "pooled_once_event_ref");
}

fn pooled_once_event_arc_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Arc::new(OnceEventPool::<Payload>::new()))
        .prepare_iter(|args| args.thread_state().bind_by_arc())
        .measure_resource_usage("pooled_once_event_arc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "pooled_once_event_arc");
}

fn pooled_once_event_ptr_st(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Box::pin(OnceEventPool::<Payload>::new()))
        .prepare_iter(|args| {
            // SAFETY: We keep the pool alive for the duration of the iteration,
            // dropping it only at the moment of cleanup of each iteration.
            let (sender, receiver) = unsafe { args.thread_state().as_ref().bind_by_ptr() };

            (sender, receiver)
        })
        .measure_resource_usage("pooled_once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|mut args| {
            let (sender, receiver) = args.take_iter_state();

            sender.send(42);
            block_on(receiver).unwrap();
        })
        .execute_criterion_on(pool, group, "pooled_once_event_ptr");
}
