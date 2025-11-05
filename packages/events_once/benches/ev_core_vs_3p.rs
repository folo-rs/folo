#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::pin::pin;
use std::task;
use std::task::Waker;

use criterion::{Criterion, criterion_group, criterion_main};
use events_once::{Event, LocalEvent};

fn entrypoint(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_vs_3p");

    g.bench_function("local_boxed_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = LocalEvent::<i32>::boxed();
            let mut receiver = pin!(receiver);

            sender.send(42);

            let mut cx = task::Context::from_waker(Waker::noop());
            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.bench_function("sync_boxed_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = Event::<i32>::boxed();
            let mut receiver = pin!(receiver);

            sender.send(42);

            let mut cx = task::Context::from_waker(Waker::noop());
            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.bench_function("oneshot_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = oneshot::channel::<i32>();
            let mut receiver = pin!(receiver);

            sender.send(42).unwrap();

            let mut cx = task::Context::from_waker(Waker::noop());
            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.bench_function("local_boxed_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = LocalEvent::<i32>::boxed();
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));
            sender.send(42);
            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.bench_function("sync_boxed_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = Event::<i32>::boxed();
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));
            sender.send(42);
            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.bench_function("oneshot_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = oneshot::channel::<i32>();
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));
            sender.send(42).unwrap();
            _ = black_box(receiver.as_mut().poll(&mut cx));
        });
    });

    g.finish();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
