//! Basic benchmarks for the `dataless_pool` crate.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::Layout;
use std::collections::VecDeque;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use dataless_pool::DatalessPool;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type TestItem = usize;
const TEST_VALUE: TestItem = 1024;

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("dp_fill");

    group.bench_function("empty", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            drop(black_box(DatalessPool::new(layout)));
        });
    });

    group.bench_function("one", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);

            let reservation = pool.reserve();

            // SAFETY: We just reserved this memory and are writing the correct type.
            unsafe {
                reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
            }

            // SAFETY: The reserved memory contains TestItem data which is Copy and has no
            // destructor, so no destructors need to be called before releasing the memory.
            unsafe {
                pool.release(reservation);
            }

            drop(pool);
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);

            let mut reservations = Vec::with_capacity(10_000);

            for _ in 0..10_000 {
                let reservation = pool.reserve();

                // SAFETY: We just reserved this memory and are writing the correct type.
                unsafe {
                    reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
                }

                reservations.push(reservation);
            }

            // SAFETY: The reserved memory contains TestItem data which is Copy and has no
            // destructor, so no destructors need to be called before releasing the memory.
            unsafe {
                for reservation in reservations {
                    pool.release(reservation);
                }
            }

            drop(pool);
        });
    });

    group.bench_function("forward_10_back_5_times_1000", |b| {
        // We add 10 items, remove the first 5 and repeat this 1000 times.
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);
            let mut all_reservations = VecDeque::with_capacity(1000 * 10);

            for _ in 0..1000 {
                for _ in 0..10 {
                    let reservation = pool.reserve();

                    // SAFETY: We just reserved this memory and are writing the correct type.
                    unsafe {
                        reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
                    }

                    all_reservations.push_back(reservation);
                }

                for _ in 0..5 {
                    let reservation = all_reservations.pop_front().unwrap();

                    // SAFETY: The reserved memory contains TestItem data which is Copy and has no
                    // destructor, so no destructors need to be called before releasing the memory.
                    unsafe {
                        pool.release(reservation);
                    }
                }
            }

            // Clean up remaining reservations.
            // SAFETY: The reserved memory contains TestItem data which is Copy and has no
            // destructor, so no destructors need to be called before releasing the memory.
            unsafe {
                while let Some(reservation) = all_reservations.pop_front() {
                    pool.release(reservation);
                }
            }
        });
    });

    group.finish();

    let mut group = c.benchmark_group("dp_read");

    group.bench_function("one", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);
            let reservation = pool.reserve();

            // SAFETY: We just reserved this memory and are writing the correct type.
            unsafe {
                reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
            }

            // SAFETY: We wrote the value above and are reading the correct type.
            let value = unsafe { reservation.ptr().cast::<TestItem>().read() };
            black_box(value);

            // SAFETY: The reserved memory contains TestItem data which is Copy and has no
            // destructor, so no destructors need to be called before releasing the memory.
            unsafe {
                pool.release(reservation);
            }
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);

            let mut reservations = Vec::with_capacity(10_000);

            for _ in 0..10_000 {
                let reservation = pool.reserve();

                // SAFETY: We just reserved this memory and are writing the correct type.
                unsafe {
                    reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
                }

                reservations.push(reservation);
            }

            let last_reservation = reservations.last().unwrap();

            // SAFETY: We wrote the value above and are reading the correct type.
            let value = unsafe { last_reservation.ptr().cast::<TestItem>().read() };
            black_box(value);

            // SAFETY: The reserved memory contains TestItem data which is Copy and has no
            // destructor, so no destructors need to be called before releasing the memory.
            unsafe {
                for reservation in reservations {
                    pool.release(reservation);
                }
            }
        });
    });

    group.finish();

    let mut group = c.benchmark_group("dp_reserve");

    group.bench_function("single_reserve", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);

            let reservation = pool.reserve();
            black_box(&reservation);

            // SAFETY: No data was written, so no cleanup needed before releasing.
            unsafe {
                pool.release(reservation);
            }
        });
    });

    group.bench_function("reserve_with_write", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);

            let reservation = pool.reserve();

            // SAFETY: We just reserved this memory and are writing the correct type.
            unsafe {
                reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
            }

            black_box(&reservation);

            // SAFETY: The reserved memory contains TestItem data which is Copy and has no
            // destructor, so no destructors need to be called before releasing the memory.
            unsafe {
                pool.release(reservation);
            }
        });
    });

    group.bench_function("reserve_write_read", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = DatalessPool::new(layout);

            let reservation = pool.reserve();

            // SAFETY: We just reserved this memory and are writing the correct type.
            unsafe {
                reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
            }

            // SAFETY: We just wrote the value above and are reading the correct type.
            let value = unsafe { reservation.ptr().cast::<TestItem>().read() };

            black_box((value, &reservation));

            // SAFETY: The reserved memory contains TestItem data which is Copy and has no
            // destructor, so no destructors need to be called before releasing the memory.
            unsafe {
                pool.release(reservation);
            }
        });
    });

    group.finish();

    let mut group = c.benchmark_group("dp_release");

    group.bench_function("single_release", |b| {
        let layout = Layout::new::<TestItem>();

        b.iter_batched(
            || {
                let mut pool = DatalessPool::new(layout);
                let reservation = pool.reserve();

                // SAFETY: We just reserved this memory and are writing the correct type.
                unsafe {
                    reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
                }

                (pool, reservation)
            },
            |(mut pool, reservation)| {
                // SAFETY: The reserved memory contains TestItem data which is Copy and has no
                // destructor, so no destructors need to be called before releasing the memory.
                unsafe {
                    pool.release(reservation);
                }
                black_box(pool);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("batch_release_100", |b| {
        let layout = Layout::new::<TestItem>();

        b.iter_batched(
            || {
                let mut pool = DatalessPool::new(layout);
                let mut reservations = Vec::with_capacity(100);

                for _ in 0..100 {
                    let reservation = pool.reserve();

                    // SAFETY: We just reserved this memory and are writing the correct type.
                    unsafe {
                        reservation.ptr().cast::<TestItem>().write(TEST_VALUE);
                    }

                    reservations.push(reservation);
                }

                (pool, reservations)
            },
            |(mut pool, reservations)| {
                // SAFETY: The reserved memory contains TestItem data which is Copy and has no
                // destructor, so no destructors need to be called before releasing the memory.
                unsafe {
                    for reservation in reservations {
                        pool.release(reservation);
                    }
                }
                black_box(pool);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}
