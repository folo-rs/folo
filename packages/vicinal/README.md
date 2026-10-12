# vicinal

Worker pool with processor-local or memory-region-local task scheduling.

Tasks execute synchronously on the processor sampled during submission by default, with
two workers per processor. A regional policy shares a fixed worker count across all
submitting processors in each memory region. Workers start on first use of a destination
and persist until shutdown.

## Example

```rust
use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

#[tokio::main]
async fn main() {
    processor_local().await;
    memory_region_local().await;
}

async fn processor_local() {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    let task1 = scheduler.spawn(|| 42);
    let _task2 = scheduler.spawn(|| println!("doing some stuff"));

    assert_eq!(task1.await, 42);

    let scheduler2 = scheduler.clone();
    let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));

    assert_eq!(task3.await.await, 55);
}

async fn memory_region_local() {
    // Workers share each region's cumulative set of submitting processors.
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
        .build();

    assert_eq!(pool.scheduler().spawn(|| 42).await, 42);
}
```

The regional count is per region, not per processor. Workers start on the first submitting
processor. New submitting processors expand that region's permitted set without adding
workers; never-submitting processors remain excluded. Membership is per pool and cumulative.
Workers refresh affinity at task boundaries, without preempting running tasks. They may
move within their permitted set even during one task, but never steal work from other regions.
`workers_per_processor(count)` is an alias for the corresponding `WorkerPolicy`; the last
policy setter wins. Counts may exceed processor counts for blocking work.

## Tradeoffs

- **Single task latency on an idle pool** is prioritized. The expectation is that tasks are
  short-lived so that the pool is often idle.
- Regional workers stay within the submitting processors. A stable, quota-appropriate
  submitter set keeps worker placement within that set; changing submitters can grow the
  cumulative set. Quotas do not adjust worker counts or remove processors. Fractional
  quotas and other process work can still cause throttling.
- Task code must preserve worker affinity. Affinity does not bind memory allocations,
  move data or guarantee cache hits. Topology and restriction refresh are not supported.

## Shutdown

Dropping the pool releases queued tasks and waits for executing tasks and workers.
Abandoned join handles disconnect and panic when awaited, even if scheduler clones remain.
Await required work before dropping the pool.

Infrastructure failures, including worker startup and affinity refresh, log the failure
and stop the entire pool. Queued tasks are abandoned; executing tasks may finish.
The pool remains poisoned and every later submission panics synchronously, including
after the pool is dropped. Task panics do not poison the pool.

## Platform support

The package is tested on the following operating systems:

* Windows 11 x64
* Windows Server 2022 x64
* Ubuntu 24.04 x64

On non-Windows non-Linux platforms (e.g. mac OS), the package retains functional worker-pool
behavior without physical locality guarantees. The fallback topology has one memory region.

## See also

More details in the [package documentation](https://docs.rs/vicinal/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
