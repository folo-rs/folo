//! Contended benchmarks for async synchronization primitives.
//!
//! Uses `par_bench` to run multiple threads competing for the same
//! lock or permits. Each benchmark is run at 1, 2, and 4 threads to
//! show the scaling behavior under contention.
//!
//! Thread count 1 serves as the uncontended baseline — the same code
//! path is exercised but without any contention overhead.

#![allow(missing_docs, reason = "benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    contended_mutex(c);
    contended_semaphore(c);
}

fn contended_mutex(c: &mut Criterion) {
    let mut group = c.benchmark_group("contended_mutex");

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

        // asynchroniz::Mutex
        {
            let mutex = asynchroniz::Mutex::boxed(0_u64);
            Run::new()
                .prepare_thread({
                    let mutex = mutex.clone();
                    move |_| mutex.clone()
                })
                .iter(|args| {
                    let mutex = args.thread_state();
                    let mut guard = futures::executor::block_on(mutex.lock());
                    *guard = guard.wrapping_add(1);
                    drop(guard);
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("asynchroniz/{name}"));
        }

        // tokio::sync::Mutex
        {
            let mutex = std::sync::Arc::new(tokio::sync::Mutex::new(0_u64));
            Run::new()
                .prepare_thread({
                    let mutex = mutex.clone();
                    move |_| mutex.clone()
                })
                .iter(|args| {
                    let mutex = args.thread_state();
                    let mut guard = futures::executor::block_on(mutex.lock());
                    *guard = guard.wrapping_add(1);
                    drop(guard);
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("tokio/{name}"));
        }

        // async_lock::Mutex
        {
            let mutex = std::sync::Arc::new(async_lock::Mutex::new(0_u64));
            Run::new()
                .prepare_thread({
                    let mutex = mutex.clone();
                    move |_| mutex.clone()
                })
                .iter(|args| {
                    let mutex = args.thread_state();
                    let mut guard = futures::executor::block_on(mutex.lock());
                    *guard = guard.wrapping_add(1);
                    drop(guard);
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("async-lock/{name}"));
        }
    }

    group.finish();
}

fn contended_semaphore(c: &mut Criterion) {
    let mut group = c.benchmark_group("contended_semaphore");

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

        // asynchroniz::Semaphore (1 permit — maximum contention)
        {
            let sem = asynchroniz::Semaphore::boxed(1);
            Run::new()
                .prepare_thread({
                    let sem = sem.clone();
                    move |_| sem.clone()
                })
                .iter(|args| {
                    let sem = args.thread_state();
                    let permit = futures::executor::block_on(sem.acquire());
                    black_box(&permit);
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("asynchroniz/{name}"));
        }

        // tokio::sync::Semaphore (1 permit)
        {
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            Run::new()
                .prepare_thread({
                    let sem = sem.clone();
                    move |_| sem.clone()
                })
                .iter(|args| {
                    let sem = args.thread_state();
                    let permit = futures::executor::block_on(sem.acquire());
                    black_box(&permit);
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("tokio/{name}"));
        }

        // async_lock::Semaphore (1 permit)
        {
            let sem = std::sync::Arc::new(async_lock::Semaphore::new(1));
            Run::new()
                .prepare_thread({
                    let sem = sem.clone();
                    move |_| sem.clone()
                })
                .iter(|args| {
                    let sem = args.thread_state();
                    let permit = futures::executor::block_on(sem.acquire());
                    black_box(&permit);
                })
                .execute_criterion_on(&mut pool, &mut group, &format!("async-lock/{name}"));
        }
    }

    group.finish();
}
