//! Compares the full lifecycle performance of `events` with other similar libraries.
//!
//! * `OnceEvent` (in different threading and binding modes)
//! * `oneshot::channel()`
//! * `futures::channel::oneshot::channel()`
//!
//! Each mechanism is exercised in both multi-threaded and single-threaded scenarios.
//!
//! Unlike the `events_send_receive_comparison` benchmarks, this measures the full
//! lifecycle including creation and binding of each event/channel within the measured
//! iteration. Pool creation is still done outside the measurement for pooled variants.
//!
//! The multi-threaded scenario:
//!
//! 1. There are two threads.
//! 2. Sender thread creates, binds, and sends to `iter` channels.
//! 3. Receiver thread creates, binds, and receives from `iter` channels.
//! 4. Measurement ends once both threads have completed their work.
//! 5. Score is average duration of both threads.
//!
//! The single-threaded variant has creation, binding, sending and receiving
//! all on the same thread, exercised sequentially.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "Arithmetic side effects are acceptable in benchmark code"
)]
#![allow(
    clippy::mutex_atomic,
    reason = "Using Mutex for simplicity in benchmark coordination"
)]
#![allow(
    clippy::drop_non_drop,
    reason = "Explicit drops for clarity in benchmarks"
)]

use std::hint::black_box;
use std::mem::MaybeUninit;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use events::{LocalOnceEvent, LocalOnceEventPool, OnceEvent, OnceEventPool};
use futures::executor::block_on;
use many_cpus::ProcessorSet;
use par_bench::{ResourceUsageExt, Run, ThreadPool};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type Payload = u128;

fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();
    let processor_time = TimeSession::new();
    let mut one_thread = ThreadPool::new(ProcessorSet::single());

    let mut group = c.benchmark_group("events_full_cycle");

    local_once_event_ref(&mut one_thread, &mut group, &allocs, &processor_time);
    local_once_event_rc(&mut one_thread, &mut group, &allocs, &processor_time);
    local_once_event_ptr(&mut one_thread, &mut group, &allocs, &processor_time);
    local_once_event_in_place_ptr(&mut one_thread, &mut group, &allocs, &processor_time);
    once_event_arc(&mut one_thread, &mut group, &allocs, &processor_time);
    once_event_ptr(&mut one_thread, &mut group, &allocs, &processor_time);
    once_event_in_place_ptr(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_local_once_event_ref(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_local_once_event_rc(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_local_once_event_ptr(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_once_event_ref(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_once_event_arc(&mut one_thread, &mut group, &allocs, &processor_time);
    pooled_once_event_ptr(&mut one_thread, &mut group, &allocs, &processor_time);
    oneshot_channel(&mut one_thread, &mut group, &allocs, &processor_time);
    futures_oneshot_channel(&mut one_thread, &mut group, &allocs, &processor_time);

    group.finish();

    allocs.print_to_stdout();
    processor_time.print_to_stdout();
}

fn local_once_event_ref(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("local_once_event_ref", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let event = LocalOnceEvent::<Payload>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "local_once_event_ref");
}

fn local_once_event_rc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("local_once_event_rc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let event = Rc::new(LocalOnceEvent::<Payload>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "local_once_event_rc");
}

fn local_once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("local_once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let event = pin!(LocalOnceEvent::<Payload>::new());

            // SAFETY: We keep the event alive for the duration of this scope.
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            sender.send(42);
            black_box(block_on(receiver).unwrap());

            // We are required to keep the event alive until both sender and receiver are gone.
            drop(event);
        })
        .execute_criterion_on(pool, group, "local_once_event_ptr");
}

fn local_once_event_in_place_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("local_once_event_in_place_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let mut event_storage = pin!(MaybeUninit::uninit());

            // SAFETY: We keep the event alive until sender/receiver are done and then we have
            // to properly drop the event.
            let (sender, receiver) =
                unsafe { LocalOnceEvent::<Payload>::new_in_place_by_ptr(event_storage.as_mut()) };

            sender.send(42);
            black_box(block_on(receiver).unwrap());

            // SAFETY: We are just removing the Pin wrapper so we can drop it because the
            // MaybeUninit type is not designed for pinned usage, so does not understand Pin.
            let event_storage_ref = unsafe { event_storage.get_unchecked_mut() };

            // SAFETY: We initialized it above and have dropped both sender and receiver now.
            unsafe {
                event_storage_ref.assume_init_drop();
            }
        })
        .execute_criterion_on(pool, group, "local_once_event_in_place_ptr");
}

fn once_event_arc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("once_event_arc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let event = Arc::new(OnceEvent::<Payload>::new());
            let (sender, receiver) = event.bind_by_arc();

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "once_event_arc");
}

fn once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let event = pin!(OnceEvent::<Payload>::new());

            // SAFETY: We keep the event alive for the duration of this scope.
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            sender.send(42);
            black_box(block_on(receiver).unwrap());

            // We are required to keep the event alive until both sender and receiver are gone.
            drop(event);
        })
        .execute_criterion_on(pool, group, "once_event_ptr");
}

fn once_event_in_place_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("once_event_in_place_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let mut event_storage = pin!(MaybeUninit::uninit());

            // SAFETY: We keep the event alive until sender/receiver are done and then we have
            // to properly drop the event.
            let (sender, receiver) =
                unsafe { OnceEvent::<Payload>::new_in_place_by_ptr(event_storage.as_mut()) };

            sender.send(42);
            black_box(block_on(receiver).unwrap());

            // We are required to keep the event alive until both sender and receiver are gone.

            // SAFETY: We are just removing the Pin wrapper so we can drop it because the
            // MaybeUninit type is not designed for pinned usage, so does not understand Pin.
            let event_storage_ref = unsafe { event_storage.get_unchecked_mut() };

            // SAFETY: We initialized it above and have dropped both sender and receiver now.
            unsafe {
                event_storage_ref.assume_init_drop();
            }
        })
        .execute_criterion_on(pool, group, "once_event_in_place_ptr");
}

fn oneshot_channel(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .measure_resource_usage("oneshot_channel", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let (sender, receiver) = oneshot::channel();

            drop(sender.send(42));
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "oneshot_channel");
}

fn futures_oneshot_channel(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    #[expect(clippy::absolute_paths, reason = "being explicit")]
    Run::new()
        .measure_resource_usage("futures_oneshot_channel", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|_| {
            let (sender, receiver) = futures::channel::oneshot::channel();

            _ = sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "futures_oneshot_channel");
}

fn pooled_local_once_event_ref(
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
            let (sender, receiver) = args.thread_state().bind_by_ref();

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_ref");
}

fn pooled_local_once_event_rc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Rc::new(LocalOnceEventPool::<Payload>::new()))
        .measure_resource_usage("pooled_local_once_event_rc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|args| {
            let (sender, receiver) = args.thread_state().bind_by_rc();

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_rc");
}

fn pooled_local_once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Box::pin(LocalOnceEventPool::<Payload>::new()))
        .measure_resource_usage("pooled_local_once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|args| {
            // SAFETY: We keep the pool alive for the duration of the iteration,
            // dropping it only at the moment of cleanup of each iteration.
            let (sender, receiver) = unsafe { args.thread_state().as_ref().bind_by_ptr() };

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "pooled_local_once_event_ptr");
}

fn pooled_once_event_ref(
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
            let (sender, receiver) = args.thread_state().bind_by_ref();

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "pooled_once_event_ref");
}

fn pooled_once_event_arc(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Arc::new(OnceEventPool::<Payload>::new()))
        .measure_resource_usage("pooled_once_event_arc", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|args| {
            let (sender, receiver) = args.thread_state().bind_by_arc();

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "pooled_once_event_arc");
}

fn pooled_once_event_ptr(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &Session,
    processor_time: &TimeSession,
) {
    Run::new()
        .prepare_thread(|_| Box::pin(OnceEventPool::<Payload>::new()))
        .measure_resource_usage("pooled_once_event_ptr", |measure| {
            measure.allocs(allocs).processor_time(processor_time)
        })
        .iter(|args| {
            // SAFETY: We keep the pool alive for the duration of the iteration,
            // dropping it only at the moment of cleanup of each iteration.
            let (sender, receiver) = unsafe { args.thread_state().as_ref().bind_by_ptr() };

            sender.send(42);
            black_box(block_on(receiver).unwrap());
        })
        .execute_criterion_on(pool, group, "pooled_once_event_ptr");
}
