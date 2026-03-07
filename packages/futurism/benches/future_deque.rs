#![expect(missing_docs, reason = "benchmarks do not require API documentation")]

use std::{
    future::Future,
    hint::black_box,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use criterion::{Criterion, criterion_group, criterion_main};
use futures::Stream;
use futurism::{FutureDeque, LocalFutureDeque};

/// A future that returns `Pending` for `remaining` polls, then `Ready(value)`.
struct CountdownFuture {
    remaining: usize,
    value: u64,
}

impl Unpin for CountdownFuture {}

impl CountdownFuture {
    fn new(remaining: usize, value: u64) -> Self {
        Self { remaining, value }
    }
}

impl Future for CountdownFuture {
    type Output = u64;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        let this = self.get_mut();
        if this.remaining == 0 {
            Poll::Ready(this.value)
        } else {
            this.remaining = this.remaining.wrapping_sub(1);
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

const FEW_ITEMS: usize = 8;
const MANY_ITEMS: usize = 1000;
const ACTIVE_RATIO_LOW: usize = 10;
const ACTIVE_RATIO_HIGH: usize = 900;

fn bench_local_future_deque(c: &mut Criterion) {
    let mut group = c.benchmark_group("local_future_deque");

    group.bench_function("few_items_all_active", |b| {
        b.iter(|| {
            let mut deque = LocalFutureDeque::new();
            for i in 0..FEW_ITEMS {
                deque.push_back(CountdownFuture::new(0, i as u64));
            }
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            for _ in 0..FEW_ITEMS {
                let result = Pin::new(&mut deque).poll_next(cx);
                let _result = black_box(result);
            }
        });
    });

    group.bench_function("many_items_mostly_inactive", |b| {
        b.iter(|| {
            let mut deque = LocalFutureDeque::new();
            for i in 0..MANY_ITEMS {
                let remaining = if i < ACTIVE_RATIO_LOW { 0 } else { 1000 };
                deque.push_back(CountdownFuture::new(remaining, i as u64));
            }
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            // Drive one round to poll all futures and activate the ones that
            // are immediately ready.
            for _ in 0..ACTIVE_RATIO_LOW {
                let result = Pin::new(&mut deque).poll_next(cx);
                let _result = black_box(result);
            }
        });
    });

    group.bench_function("many_items_mostly_active", |b| {
        b.iter(|| {
            let mut deque = LocalFutureDeque::new();
            for i in 0..MANY_ITEMS {
                let remaining = if i < ACTIVE_RATIO_HIGH { 0 } else { 1000 };
                deque.push_back(CountdownFuture::new(remaining, i as u64));
            }
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            for _ in 0..ACTIVE_RATIO_HIGH {
                let result = Pin::new(&mut deque).poll_next(cx);
                let _result = black_box(result);
            }
        });
    });

    group.finish();
}

fn bench_future_deque(c: &mut Criterion) {
    let mut group = c.benchmark_group("future_deque");

    group.bench_function("few_items_all_active", |b| {
        b.iter(|| {
            let mut deque = FutureDeque::new();
            for i in 0..FEW_ITEMS {
                deque.push_back(CountdownFuture::new(0, i as u64));
            }
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            for _ in 0..FEW_ITEMS {
                let result = Pin::new(&mut deque).poll_next(cx);
                let _result = black_box(result);
            }
        });
    });

    group.bench_function("many_items_mostly_inactive", |b| {
        b.iter(|| {
            let mut deque = FutureDeque::new();
            for i in 0..MANY_ITEMS {
                let remaining = if i < ACTIVE_RATIO_LOW { 0 } else { 1000 };
                deque.push_back(CountdownFuture::new(remaining, i as u64));
            }
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            for _ in 0..ACTIVE_RATIO_LOW {
                let result = Pin::new(&mut deque).poll_next(cx);
                let _result = black_box(result);
            }
        });
    });

    group.bench_function("many_items_mostly_active", |b| {
        b.iter(|| {
            let mut deque = FutureDeque::new();
            for i in 0..MANY_ITEMS {
                let remaining = if i < ACTIVE_RATIO_HIGH { 0 } else { 1000 };
                deque.push_back(CountdownFuture::new(remaining, i as u64));
            }
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            for _ in 0..ACTIVE_RATIO_HIGH {
                let result = Pin::new(&mut deque).poll_next(cx);
                let _result = black_box(result);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_local_future_deque, bench_future_deque);
criterion_main!(benches);
