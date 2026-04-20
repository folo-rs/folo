//! Contended benchmarks for async event primitives.
//!
//! Uses `par_bench` to run multiple threads competing on the same
//! event. All threads perform `set` + `try_wait` (+ `reset` for manual)
//! cycles, measuring lock contention as thread count scales.
//!
//! `rsevents` is excluded because its blocking `.wait()` API is not
//! comparable in a multi-threaded contention benchmark (threads
//! block on OS waits rather than contending on a lock).

#![allow(missing_docs, reason = "benchmark code")]

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use event_listener::Event as ElEvent;
use many_cpus::SystemHardware;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    contended_auto_reset(c);
    contended_manual_reset(c);
}

/// Contended benchmark for auto-reset events.
///
/// All threads perform `set()` + `try_wait()` cycles on the same event.
/// This measures contention on the internal synchronization when
/// multiple threads access the event state concurrently.
fn contended_auto_reset(c: &mut Criterion) {
    let mut group = c.benchmark_group("contended_auto_reset");

    let processors = SystemHardware::current().processors();

    for thread_count in [1, 2, 4] {
        let Some(subset) = processors
            .to_builder()
            .take(thread_count.try_into().unwrap())
        else {
            continue;
        };
        let mut pool = ThreadPool::new(subset);

        let name = format!("{thread_count}_threads");

        // events::AutoResetEvent
        {
            let event = events::AutoResetEvent::boxed();
            Run::new()
                .prepare_thread({
                    let event = event.clone();
                    move |_| event.clone()
                })
                .iter(|args| {
                    let event = args.thread_state();
                    event.set();
                    black_box(event.try_wait());
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("events/{name}"));
        }

        // event-listener (notify + try_listen pattern)
        {
            let event = Arc::new(ElEvent::new());
            Run::new()
                .prepare_thread({
                    let event = Arc::clone(&event);
                    move |_| Arc::clone(&event)
                })
                .iter(|args| {
                    let event = args.thread_state();
                    event.notify(1);
                    // event-listener has no try_wait; just notify
                    // as the closest equivalent operation.
                    black_box(event.total_listeners());
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("event-listener/{name}"));
        }
    }

    group.finish();
}

/// Contended benchmark for manual-reset events.
///
/// All threads perform `set()` + `try_wait()` + `reset()` cycles on the
/// same event. This measures contention on the internal
/// synchronization.
fn contended_manual_reset(c: &mut Criterion) {
    let mut group = c.benchmark_group("contended_manual_reset");

    let processors = SystemHardware::current().processors();

    for thread_count in [1, 2, 4] {
        let Some(subset) = processors
            .to_builder()
            .take(thread_count.try_into().unwrap())
        else {
            continue;
        };
        let mut pool = ThreadPool::new(subset);

        let name = format!("{thread_count}_threads");

        // events::ManualResetEvent
        {
            let event = events::ManualResetEvent::boxed();
            Run::new()
                .prepare_thread({
                    let event = event.clone();
                    move |_| event.clone()
                })
                .iter(|args| {
                    let event = args.thread_state();
                    event.set();
                    black_box(event.try_wait());
                    event.reset();
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("events/{name}"));
        }

        // event-listener (notify_additional + total_listeners)
        {
            let event = Arc::new(ElEvent::new());
            Run::new()
                .prepare_thread({
                    let event = Arc::clone(&event);
                    move |_| Arc::clone(&event)
                })
                .iter(|args| {
                    let event = args.thread_state();
                    event.notify(usize::MAX);
                    black_box(event.total_listeners());
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("event-listener/{name}"));
        }
    }

    group.finish();
}
