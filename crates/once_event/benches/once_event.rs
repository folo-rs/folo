//! Benchmarks for the `once_event` crate.

#![allow(
    missing_docs,
    reason = "Benchmarks do not require public documentation"
)]

use std::task;

use criterion::{Criterion, criterion_group, criterion_main};
use futures::FutureExt;
use futures::task::noop_waker_ref;
use once_event::{OnceEventEmbedded, OnceEventPoolByRc, OnceEventPoolByRef, OnceEventPoolUnsafe};

criterion_group!(benches, once_event);
criterion_main!(benches);

const CANARY: usize = 0x356789111aaaa;

fn once_event(c: &mut Criterion) {
    let mut group = c.benchmark_group("once_event");

    let ref_storage = OnceEventPoolByRef::<usize>::new();
    let rc_storage = OnceEventPoolByRc::<usize>::new();
    let unsafe_storage = Box::pin(OnceEventPoolUnsafe::<usize>::new());

    let cx = &mut task::Context::from_waker(noop_waker_ref());

    group.bench_function("ref_getsetget", |b| {
        b.iter(|| {
            let (sender, mut receiver) = ref_storage.activate();

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Pending);

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.bench_function("rc_getsetget", |b| {
        b.iter(|| {
            let (sender, mut receiver) = rc_storage.activate();

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Pending);

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.bench_function("unsafe_getsetget", |b| {
        b.iter(|| {
            // SAFETY: The unsafe_storage is a valid pinned Box that outlives this closure.
            let (sender, mut receiver) = unsafe { unsafe_storage.as_ref().activate() };

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Pending);

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.bench_function("embedded_getsetget", |b| {
        b.iter(|| {
            // We need a fresh embedded storage for each iteration since it can only be activated once
            let mut storage = Box::pin(OnceEventEmbedded::<usize>::new());
            let (sender, mut receiver) = storage.as_mut().activate();

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Pending);

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.bench_function("ref_setget", |b| {
        b.iter(|| {
            let (sender, mut receiver) = ref_storage.activate();

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.bench_function("rc_setget", |b| {
        b.iter(|| {
            let (sender, mut receiver) = rc_storage.activate();

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.bench_function("unsafe_setget", |b| {
        b.iter(|| {
            // SAFETY: The unsafe_storage is a valid pinned Box that outlives this closure.
            let (sender, mut receiver) = unsafe { unsafe_storage.as_ref().activate() };

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.bench_function("embedded_setget", |b| {
        b.iter(|| {
            // We need a fresh embedded storage for each iteration since it can only be activated once
            let mut storage = Box::pin(OnceEventEmbedded::<usize>::new());
            let (sender, mut receiver) = storage.as_mut().activate();

            sender.set(CANARY);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(CANARY));
        });
    });

    group.finish();
}
