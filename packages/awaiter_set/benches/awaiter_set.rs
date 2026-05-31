//! Wall-clock benchmarks for the `awaiter_set` crate.
//!
//! Round-trip scenarios cover the same operations isolated by
//! `awaiter_set_cg.rs`:
//!
//! * `register_unregister/empty` — register + unregister on an empty set.
//! * `register_unregister/with_10_anchors` — same, but the set has 10
//!   anchor awaiters that remain registered for the duration.
//! * `register_notify_take/empty` — full lifecycle: register, notify, take.
//! * `is_empty/empty` — null-head fast path on an empty set.
//! * `is_empty/populated` — null-head check on a populated set.
//! * `notify_one_prior_generation/eligible` — register, advance generation,
//!   notify prior generation, take notification.

#![allow(missing_docs, reason = "benchmark code")]
#![allow(
    clippy::let_underscore_must_use,
    reason = "results are intentionally discarded in benchmarks"
)]
#![allow(
    clippy::undocumented_unsafe_blocks,
    reason = "benchmark pinning has trivial safety invariants"
)]
#![allow(
    clippy::multiple_unsafe_ops_per_block,
    reason = "benchmarks group adjacent register/unregister calls"
)]

use std::hint::black_box;
use std::iter;
use std::pin::Pin;
use std::task::Waker;
use std::time::Instant;

use awaiter_set::{Awaiter, AwaiterSet};
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    register_unregister(c);
    register_notify_take(c);
    is_empty(c);
    notify_one_prior_generation(c);
}

/// Measures back-to-back `register` + `unregister` on a single
/// reusable awaiter. The set ends each iteration in the same state it
/// started, allowing the awaiter and set to be reused across all
/// iterations of one sample.
fn register_unregister(c: &mut Criterion) {
    let mut group = c.benchmark_group("register_unregister");

    group.bench_function("empty", |b| {
        b.iter_custom(|iters| {
            let mut awaiter = Box::pin(Awaiter::new());
            let mut set = AwaiterSet::new();
            let start = Instant::now();
            for _ in 0..iters {
                unsafe {
                    set.register(awaiter.as_mut(), Waker::noop().clone());
                    set.unregister(awaiter.as_mut());
                }
            }
            let elapsed = start.elapsed();
            black_box(&mut set);
            black_box(&mut awaiter);
            elapsed
        });
    });

    group.bench_function("with_10_anchors", |b| {
        b.iter_custom(|iters| {
            let mut anchors: Vec<Pin<Box<Awaiter>>> =
                iter::repeat_with(|| Box::pin(Awaiter::new()))
                    .take(10)
                    .collect();
            let mut set = AwaiterSet::new();
            for anchor in &mut anchors {
                unsafe {
                    set.register(anchor.as_mut(), Waker::noop().clone());
                }
            }
            let mut target = Box::pin(Awaiter::new());

            let start = Instant::now();
            for _ in 0..iters {
                unsafe {
                    set.register(target.as_mut(), Waker::noop().clone());
                    set.unregister(target.as_mut());
                }
            }
            let elapsed = start.elapsed();
            black_box(&mut set);
            black_box(&mut anchors);
            black_box(&mut target);
            elapsed
        });
    });

    group.finish();
}

/// Measures the full async lifecycle round-trip on a single reusable
/// awaiter: register, notify (returns waker), take the notification.
/// Net state change per iteration is zero, allowing reuse across the
/// whole sample.
fn register_notify_take(c: &mut Criterion) {
    let mut group = c.benchmark_group("register_notify_take");

    group.bench_function("empty", |b| {
        b.iter_custom(|iters| {
            let mut awaiter = Box::pin(Awaiter::new());
            let mut set = AwaiterSet::new();
            let start = Instant::now();
            for _ in 0..iters {
                unsafe {
                    set.register(awaiter.as_mut(), Waker::noop().clone());
                }
                drop(black_box(set.notify_one()));
                _ = black_box(awaiter.as_ref().take_notification());
            }
            let elapsed = start.elapsed();
            black_box(&mut set);
            black_box(&mut awaiter);
            elapsed
        });
    });

    group.finish();
}

/// Measures `is_empty` on an empty and a populated set. State is
/// invariant across iterations, so the set and awaiter are constructed
/// once per sample.
fn is_empty(c: &mut Criterion) {
    let mut group = c.benchmark_group("is_empty");

    group.bench_function("empty", |b| {
        b.iter_custom(|iters| {
            let set = AwaiterSet::new();
            let start = Instant::now();
            for _ in 0..iters {
                _ = black_box(black_box(&set).is_empty());
            }
            let elapsed = start.elapsed();
            black_box(&set);
            elapsed
        });
    });

    group.bench_function("populated", |b| {
        b.iter_custom(|iters| {
            let mut awaiter = Box::pin(Awaiter::new());
            let mut set = AwaiterSet::new();
            unsafe {
                set.register(awaiter.as_mut(), Waker::noop().clone());
            }
            let start = Instant::now();
            for _ in 0..iters {
                _ = black_box(black_box(&set).is_empty());
            }
            let elapsed = start.elapsed();
            black_box(&mut set);
            black_box(&mut awaiter);
            elapsed
        });
    });

    group.finish();
}

/// Measures a full round-trip exercising
/// `advance_generation` + `notify_one_prior_generation`: register the
/// awaiter in the current generation, advance into a new generation
/// (the awaiter is now in a prior generation), notify it, then take
/// the notification to reset state for the next iteration.
fn notify_one_prior_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("notify_one_prior_generation");

    group.bench_function("eligible", |b| {
        b.iter_custom(|iters| {
            let mut awaiter = Box::pin(Awaiter::new());
            let mut set = AwaiterSet::new();
            let start = Instant::now();
            for _ in 0..iters {
                unsafe {
                    set.register(awaiter.as_mut(), Waker::noop().clone());
                }
                set.advance_generation();
                drop(black_box(set.notify_one_prior_generation()));
                _ = black_box(awaiter.as_ref().take_notification());
            }
            let elapsed = start.elapsed();
            black_box(&mut set);
            black_box(&mut awaiter);
            elapsed
        });
    });

    group.finish();
}
