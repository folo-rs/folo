#![expect(
    missing_docs,
    clippy::undocumented_unsafe_blocks,
    reason = "benchmarks"
)]

use std::hint::black_box;
use std::pin::pin;
use std::task;
use std::task::{Poll, Waker};

use criterion::{Criterion, criterion_group, criterion_main};
use events_once::{
    Event, EventLake, EventPool, LocalEvent, LocalEventLake, LocalEventPool, RawEventLake,
    RawEventPool, RawLocalEventLake, RawLocalEventPool,
};

fn entrypoint(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_vs_3p");

    let local_pool = LocalEventPool::<i32>::new();
    let sync_pool = EventPool::<i32>::new();
    let local_raw_pool = pin!(RawLocalEventPool::<i32>::new());
    let sync_raw_pool = pin!(RawEventPool::<i32>::new());
    let local_lake = LocalEventLake::new();
    let sync_lake = EventLake::new();
    let local_raw_lake = pin!(RawLocalEventLake::new());
    let sync_raw_lake = pin!(RawEventLake::new());

    g.bench_function("local_boxed_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(LocalEvent::<i32>::boxed());
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_boxed_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(Event::<i32>::boxed());
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_pooled_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(local_pool.rent());
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_pooled_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(sync_pool.rent());
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_raw_pooled_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { local_raw_pool.as_ref().rent() });
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_raw_pooled_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { sync_raw_pool.as_ref().rent() });
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_lake_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(local_lake.rent());
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_lake_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(sync_lake.rent());
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_raw_lake_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { local_raw_lake.rent() });
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_raw_lake_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { sync_raw_lake.rent() });
            let mut receiver = pin!(receiver);

            sender.send(black_box(42));

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("oneshot_send_receive", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(oneshot::channel::<i32>());
            let mut receiver = pin!(receiver);

            sender.send(black_box(42)).unwrap();

            let mut cx = task::Context::from_waker(Waker::noop());
            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.finish();

    let mut g = c.benchmark_group("events_once_vs_3p_2poll");

    g.bench_function("local_boxed_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(LocalEvent::<i32>::boxed());
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_boxed_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(Event::<i32>::boxed());
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_pooled_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(local_pool.rent());
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_pooled_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(sync_pool.rent());
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_raw_pooled_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { local_raw_pool.as_ref().rent() });
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_raw_pooled_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { sync_raw_pool.as_ref().rent() });
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_lake_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(local_lake.rent());
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_lake_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(sync_lake.rent());
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("local_raw_lake_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { local_raw_lake.rent() });
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("sync_raw_lake_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(unsafe { sync_raw_lake.rent() });
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42));

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.bench_function("oneshot_send_receive_2poll", |b| {
        b.iter(|| {
            let (sender, receiver) = black_box(oneshot::channel::<i32>());
            let mut receiver = pin!(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());

            _ = black_box(receiver.as_mut().poll(&mut cx));

            sender.send(black_box(42)).unwrap();

            assert_eq!(
                black_box(receiver.as_mut().poll(&mut cx)),
                Poll::Ready(Ok(42))
            );
        });
    });

    g.finish();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
