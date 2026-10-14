# vicinal

Worker pool with configurable processor affinity.

Processor affinity is the set of processors on which a worker is permitted to run.

A memory region groups processors with their local physical memory, as reported by
[`many_cpus`](https://docs.rs/many_cpus/). Access to local memory can be faster than access
across regions. Many systems have only one region.

A caller submits a task through one of a pool's `Scheduler::spawn*` methods. The pool samples
the calling thread's current logical processor; this is the task's submitting processor.
Tasks execute synchronously on that processor by default.
A regional policy shares a fixed worker count across submitting processors in each memory
region. A system policy shares a fixed count across all submitting processors, regardless
of region. Workers start on first use of a destination by default and persist until shutdown.
Opt into eager initialization to prepare selected destinations during pool construction.

## Example

The example uses `ohno::AppError`; enable the `app-err` feature on `ohno`.

```rust
use many_cpus::SystemHardware;
use new_zealand::nz;
use ohno::AppError;
use vicinal::{Pool, TaskError, WorkerPolicy};

#[tokio::main]
async fn main() -> Result<(), AppError> {
    processor_local().await?;
    memory_region_local().await?;
    system_wide().await?;
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
    // Use this same selection for the application's submitting threads.
    let processors = SystemHardware::current().processors();
    // Illustrative concurrency; choose the count for your blocking workload.
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::per_memory_region(nz!(2_u32)))
        .eager(processors)
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

/// Demonstrates a worker count independent of the submitting processors' memory regions.
async fn system_wide() -> Result<(), AppError> {
    // Illustrative concurrency shared across every submitting processor in this pool.
    let pool = Pool::builder()
        .worker_policy(WorkerPolicy::per_system(nz!(2_u32)))
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

The regional count is per region, not per processor. The system count is per pool, not per
region. Shared workers start with the eagerly selected processors in their destination,
or on the first submitting processor.
New submitting processors expand the destination's permitted set without adding
workers; processors neither selected nor submitting remain excluded.
Membership is per pool and cumulative.
Workers refresh affinity at task boundaries, without preempting running tasks. They may
move within their permitted set even during one task. Regional workers never steal work from
other regions; system workers share all regions' work.
Configure concurrency with `WorkerPolicy`; counts may exceed processor counts for blocking work.
Each pool counts workers independently, while scheduler clones share their pool's workers.
Regions describe processor locality, not the task's allocations or data.

## Eager initialization

`PoolBuilder::eager(processors)` takes ownership of a nonempty `ProcessorSet` from the pool's
hardware instance. The setter starts no workers. `build()` starts the configured workers for
every selected processor, represented region or the system, according to the worker policy.
It waits for initial affinity and every configured `thread_init` callback to complete.
Initialization effects are visible when construction returns, and the calling thread's affinity
remains unchanged. Startup failure stops and joins created workers before construction panics.

Callbacks run once per worker, may overlap, and must preserve affinity. They must not wait for
same-pool work, shut down their pool or depend on eager construction returning. In particular,
an eager initializer cannot depend on the caller publishing the pool's scheduler after `build()`.

The selection is initial membership, not a permanent limit. Later submissions from other eligible
processors still create missing destinations or expand shared affinity. Choose a processor set
once and reuse it for the application and pool rather than independently choosing quota-sized
subsets. Eager construction prepares workers, not task-specific allocations or application caches.

## Tradeoffs

- **Single task latency on an idle pool** is prioritized. The expectation is that tasks are
  short-lived so that the pool is often idle.
- Shared workers stay within eagerly selected processors and accepted submitters. Using one stable,
  quota-appropriate processor set for selection and submissions keeps workers within it.
  Using new processors grows the cumulative set. Quotas do not adjust worker counts or remove processors. Fractional
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
