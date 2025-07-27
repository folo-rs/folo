//! Compares the create/destroy overhead of `events` with other similar libraries.
//!
//! * `OnceEvent` (in different threading and binding modes)
//! * `oneshot::channel()`
//! * `futures::channel::oneshot::channel()`
//!
//! In each benchmark, a channel/event is created (as well as a sender/receiver bound, where
//! applicable) and then immediately destroyed.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use events::{LocalOnceEvent, LocalOnceEventPool, OnceEvent, OnceEventPool};
use many_cpus::ProcessorSet;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type Payload = u128;

fn entrypoint(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(&ProcessorSet::single());

    let mut group = c.benchmark_group("events_overhead_comparison");

    Run::new()
        .iter(|_| {
            let event = LocalOnceEvent::<Payload>::new();
            drop(event.bind_by_ref());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "local_once_event_ref");

    Run::new()
        .iter(|_| {
            let event = Rc::new(LocalOnceEvent::<Payload>::new());
            drop(event.bind_by_rc());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "local_once_event_rc");

    Run::new()
        .iter(|_| {
            let event = pin!(LocalOnceEvent::<Payload>::new());
            // SAFETY: We are immediately dropping the sender/receiver, so `event` outlives them.
            unsafe {
                drop(event.as_ref().bind_by_ptr());
            }
        })
        .execute_criterion_on(&mut one_thread, &mut group, "local_once_event_ptr");

    Run::new()
        .iter(|_| {
            let event = OnceEvent::<Payload>::new();
            drop(event.bind_by_ref());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "once_event_ref");

    Run::new()
        .iter(|_| {
            let event = Arc::new(OnceEvent::<Payload>::new());
            drop(event.bind_by_arc());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "once_event_arc");

    Run::new()
        .iter(|_| {
            let event = Box::pin(OnceEvent::<Payload>::new());
            // SAFETY: We are immediately dropping the sender/receiver, so `event` outlives them.
            unsafe {
                drop(event.as_ref().bind_by_ptr());
            }
        })
        .execute_criterion_on(&mut one_thread, &mut group, "once_event_ptr");

    Run::new()
        .prepare_thread(|_| LocalOnceEventPool::<Payload>::new())
        .iter(|args| {
            drop(args.thread_state().bind_by_ref());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "pooled_local_once_event_ref");

    Run::new()
        .prepare_thread(|_| Rc::new(LocalOnceEventPool::<Payload>::new()))
        .iter(|args| {
            drop(args.thread_state().bind_by_rc());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "pooled_local_once_event_rc");

    Run::new()
        .prepare_thread(|_| Box::pin(LocalOnceEventPool::<Payload>::new()))
        .iter(|args| {
            // SAFETY: We are immediately dropping the sender/receiver, so the pool outlives them.
            unsafe {
                drop(args.thread_state().as_ref().bind_by_ptr());
            }
        })
        .execute_criterion_on(&mut one_thread, &mut group, "pooled_local_once_event_ptr");

    Run::new()
        .prepare_thread(|_| OnceEventPool::<Payload>::new())
        .iter(|args| {
            drop(args.thread_state().bind_by_ref());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "pooled_once_event_ref");

    Run::new()
        .prepare_thread(|_| Arc::new(OnceEventPool::<Payload>::new()))
        .iter(|args| {
            drop(args.thread_state().bind_by_arc());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "pooled_once_event_arc");

    Run::new()
        .prepare_thread(|_| Box::pin(OnceEventPool::<Payload>::new()))
        .iter(|args| {
            // SAFETY: We are immediately dropping the sender/receiver, so the pool outlives them.
            unsafe {
                drop(args.thread_state().as_ref().bind_by_ptr());
            }
        })
        .execute_criterion_on(&mut one_thread, &mut group, "pooled_once_event_ptr");

    Run::new()
        .iter(|_| {
            let (sender, receiver) = oneshot::channel::<Payload>();
            drop(sender);
            drop(receiver);
        })
        .execute_criterion_on(&mut one_thread, &mut group, "oneshot_channel");

    #[expect(clippy::absolute_paths, reason = "being explicit")]
    Run::new()
        .iter(|_| {
            let (sender, receiver) = futures::channel::oneshot::channel::<Payload>();
            drop(sender);
            drop(receiver);
        })
        .execute_criterion_on(&mut one_thread, &mut group, "futures_oneshot_channel");

    group.finish();
}
