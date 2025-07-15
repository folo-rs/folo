//! Compares the performance of `events` with other similar libraries.
//!
//! * `OnceEvent` (arc)
//! * `OnceEvent` (ptr)
//! * `oneshot::channel()`
//! * `futures::channel::oneshot::channel()`
//!
//! The scenario is simple:
//!
//! 1. There are two threads.
//! 2. Sender thread sends a value to `iter` channels.
//! 3. Receiver thread receives a value from `iter` channels.
//! 4. Measurement ends once both threads have completed their work.
//! 5. Score is average duration of both threads.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::sync::{Arc, LazyLock, Mutex};

use benchmark_utils::{AbWorker, ThreadPool, bench_on_threadpool_ab};
use criterion::{Bencher, Criterion, criterion_group, criterion_main};
use events::OnceEvent;
use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type Payload = u128;

static TWO_THREADS: LazyLock<ThreadPool> = LazyLock::new(|| {
    let processors = ProcessorSet::builder()
        .performance_processors_only()
        .take(nz!(2))
        .unwrap();

    ThreadPool::new(&processors)
});

/// Compares the performance of events package vs pure oneshot channels.
fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("events_comparison");

    group.bench_function("once_event_arc", once_event_arc);
    group.bench_function("once_event_ptr", once_event_ptr);
    group.bench_function("oneshot_channel", oneshot_channel);
    group.bench_function("futures_oneshot_channel", futures_oneshot_channel);

    group.finish();
}

fn once_event_arc(b: &mut Bencher<'_>) {
    b.iter_custom(|iters| {
        let mut events = Vec::with_capacity(iters as usize);

        for _ in 0..iters {
            events.push(Arc::new(OnceEvent::<Payload>::new()));
        }

        let (senders, receivers): (Vec<_>, Vec<_>) =
            events.into_iter().map(|e| e.bind_by_arc()).unzip();

        let senders = Arc::new(Mutex::new(senders));
        let receivers = Arc::new(Mutex::new(receivers));

        // TODO: This is a bit janky... prepare() should be per iteration, otherwise we are
        // spending energy popping stuff from the shared list etc.

        bench_on_threadpool_ab(
            &TWO_THREADS,
            iters,
            {
                let senders = Arc::clone(&senders);
                let receivers = Arc::clone(&receivers);
                move |ab| match ab {
                    AbWorker::A => (Some(Arc::clone(&senders)), None),
                    AbWorker::B => (None, Some(Arc::clone(&receivers))),
                }
            },
            |ab, payload| match ab {
                AbWorker::A => {
                    let sender = payload.0.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    sender.send(42);
                }
                AbWorker::B => {
                    let receiver = payload.1.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    block_on(receiver).unwrap();
                }
            },
        )
    });
}

fn once_event_ptr(b: &mut Bencher<'_>) {
    b.iter_custom(|iters| {
        let mut events = Vec::with_capacity(iters as usize);

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

        let senders = Arc::new(Mutex::new(senders));
        let receivers = Arc::new(Mutex::new(receivers));

        // TODO: This is a bit janky... prepare() should be per iteration, otherwise we are
        // spending energy popping stuff from the shared list etc.

        bench_on_threadpool_ab(
            &TWO_THREADS,
            iters,
            {
                let senders = Arc::clone(&senders);
                let receivers = Arc::clone(&receivers);
                move |ab| match ab {
                    AbWorker::A => (Some(Arc::clone(&senders)), None),
                    AbWorker::B => (None, Some(Arc::clone(&receivers))),
                }
            },
            |ab, payload| match ab {
                AbWorker::A => {
                    let sender = payload.0.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    sender.send(42);
                }
                AbWorker::B => {
                    let receiver = payload.1.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    block_on(receiver).unwrap();
                }
            },
        )
    });
}

fn oneshot_channel(b: &mut Bencher<'_>) {
    b.iter_custom(|iters| {
        let mut senders = Vec::with_capacity(iters as usize);
        let mut receivers = Vec::with_capacity(iters as usize);

        for _ in 0..iters {
            let (sender, receiver) = oneshot::channel();
            senders.push(sender);
            receivers.push(receiver);
        }

        let senders = Arc::new(Mutex::new(senders));
        let receivers = Arc::new(Mutex::new(receivers));

        bench_on_threadpool_ab(
            &TWO_THREADS,
            iters,
            {
                let senders = Arc::clone(&senders);
                let receivers = Arc::clone(&receivers);
                move |ab| match ab {
                    AbWorker::A => (Some(Arc::clone(&senders)), None),
                    AbWorker::B => (None, Some(Arc::clone(&receivers))),
                }
            },
            |ab, payload| match ab {
                AbWorker::A => {
                    let sender = payload.0.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    sender.send(42).unwrap();
                }
                AbWorker::B => {
                    let receiver = payload.1.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    block_on(receiver).unwrap();
                }
            },
        )
    });
}

fn futures_oneshot_channel(b: &mut Bencher<'_>) {
    b.iter_custom(|iters| {
        let mut senders = Vec::with_capacity(iters as usize);
        let mut receivers = Vec::with_capacity(iters as usize);

        for _ in 0..iters {
            #[expect(clippy::absolute_paths, reason = "being explicit")]
            let (sender, receiver) = futures::channel::oneshot::channel();
            senders.push(sender);
            receivers.push(receiver);
        }

        let senders = Arc::new(Mutex::new(senders));
        let receivers = Arc::new(Mutex::new(receivers));

        bench_on_threadpool_ab(
            &TWO_THREADS,
            iters,
            {
                let senders = Arc::clone(&senders);
                let receivers = Arc::clone(&receivers);
                move |ab| match ab {
                    AbWorker::A => (Some(Arc::clone(&senders)), None),
                    AbWorker::B => (None, Some(Arc::clone(&receivers))),
                }
            },
            |ab, payload| match ab {
                AbWorker::A => {
                    let sender = payload.0.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    sender.send(42).unwrap();
                }
                AbWorker::B => {
                    let receiver = payload.1.as_ref().unwrap().lock().unwrap().pop().unwrap();
                    block_on(receiver).unwrap();
                }
            },
        )
    });
}
