# vicinal

Worker pool with processor-local or memory-region-local task scheduling.

A memory region groups processors with their local physical memory, as reported by
[`many_cpus`](https://docs.rs/many_cpus/). Access to local memory can be faster than access
across regions. Many systems have only one region.

Tasks execute synchronously on the processor sampled during submission by default.
A regional policy shares a fixed worker count across all
submitting processors in each memory region. Workers start on first use of a destination
and persist until shutdown.

## Example

The example uses `ohno::AppError`; enable the `app-err` feature on `ohno`.

```rust
use new_zealand::nz;
use ohno::AppError;
use vicinal::{Pool, TaskError, WorkerPolicy};

#[tokio::main]
async fn main() -> Result<(), AppError> {
    processor_local().await?;
    memory_region_local().await?;
    Ok(())
}

async fn processor_local() -> Result<(), AppError> {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    let task1 = scheduler.spawn(|| 42);
    let _task2 = scheduler.spawn(|| println!("doing some stuff"));

    assert_eq!(task1.await.map_err(TaskError::into_diagnostic)?, 42);

    let scheduler2 = scheduler.clone();
    let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));

    let task3 = task3.await.map_err(TaskError::into_diagnostic)?;
    assert_eq!(task3.await.map_err(TaskError::into_diagnostic)?, 55);
    Ok(())
}

async fn memory_region_local() -> Result<(), AppError> {
    // Workers share each region's cumulative set of submitting processors.
    // Illustrative concurrency; choose the count for your blocking workload.
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::per_memory_region(nz!(2_u32)))
        .build();

    assert_eq!(
        pool.scheduler()
            .spawn(|| 42)
            .await
            .map_err(TaskError::into_diagnostic)?,
        42
    );
    Ok(())
}
```

The regional count is per region, not per processor. Workers start on the first submitting
processor. New submitting processors expand that region's permitted set without adding
workers; never-submitting processors remain excluded. Membership is per pool and cumulative.
Workers refresh affinity at task boundaries, without preempting running tasks. They may
move within their permitted set even during one task, but never steal work from other regions.
Configure concurrency with `WorkerPolicy`; counts may exceed processor counts for blocking work.
Each pool counts workers independently, while scheduler clones share their pool's workers.
Regions describe processor locality, not the task's allocations or data.

## Tradeoffs

- **Single task latency on an idle pool** is prioritized. The expectation is that tasks are
  short-lived so that the pool is often idle.
- Regional workers stay within the submitting processors. A stable, quota-appropriate
  submitter set keeps worker placement within that set; changing submitters can grow the
  cumulative set. Quotas do not adjust worker counts or remove processors. Fractional
  quotas and other process work can still cause throttling: operating-system delays after
  exceeding the process's processor-time allowance. See
  [avoiding operating system quota penalties].
- Task code must preserve worker affinity. Affinity does not bind memory allocations,
  move data or guarantee cache hits. Topology and restriction refresh are not supported.

## Task outcomes

Awaiting a join handle returns `Result<R, TaskError<R>>`:

- **Success:** `Ok(value)` contains the task's return value.
- **Cancellation:** `is_cancel()` identifies cancellation before execution.
  `into_body()` recovers the task as `impl FnOnce() -> R + Send + 'static` for execution elsewhere.
- **Panic:** `is_panic()` identifies a task panic.
  `into_panic()` recovers the original panic payload.

Calling a recovery method for the other outcome returns the unchanged error.

When recovery is unnecessary, `into_diagnostic()` drops the body or panic payload and returns
a `Send + Sync` error retaining the message, source chain, backtrace and enrichment. This supports
application-level error reporting without turning the task error into a string.

Dropping a handle does not cancel the task.

## Shutdown

Dropping the pool cancels queued tasks and waits for executing tasks and workers.
Cancelled joined tasks retain their captures for recovery until the handle, error or recovered
body is dropped or the body executes. Fire-and-forget cancellation releases captures.
Retained schedulers return cancelled join handles for submissions after clean shutdown.
Await required work before dropping the pool.

Infrastructure failures, including worker startup and affinity refresh, log the failure
and stop the entire pool. Queued joined tasks are cancelled recoverably; queued fire-and-forget
tasks release their captures. Executing tasks may finish.
The pool remains poisoned and every later submission panics synchronously, including
after the pool is dropped. Task panics do not poison the pool.

## Platform support

The package is tested on the following operating systems:

* Windows 11 x64
* Windows Server 2022 x64
* Ubuntu 24.04 x64

On non-Windows non-Linux platforms (e.g. macOS), the package retains functional worker-pool
behavior without physical locality guarantees. The fallback topology has one memory region.

## See also

More details in the [package documentation](https://docs.rs/vicinal/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.

[avoiding operating system quota penalties]:
  https://docs.rs/many_cpus/latest/many_cpus/#avoiding-operating-system-quota-penalties
