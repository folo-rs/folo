# vicinal design

`vicinal` executes synchronous tasks on worker threads. A caller submits a task by calling
one of a pool's `Scheduler::spawn*` methods. The pool samples the calling thread's current
logical processor during that call; this is the task's submitting processor.

Callers choose whether locality means the same logical processor, the same memory region
or a shared set of processors across the system. Processor affinity is
the set of processors on which a worker is permitted to run.

## Worker policies

`WorkerPolicy` is an opaque configuration value. Constructors select a fixed worker count
per processor, per memory region or per system, without a policy trait or generic pool and
scheduler types. Counts and processor membership are independent for each pool.
Additional constructors can extend configuration without exposing its representation.

The default is processor-local scheduling. Applications configure explicit counts through
`WorkerPolicy` and the builder's `worker_policy(...)` setter. Each call replaces the selected
policy, so the last setter wins. A built pool's policy is immutable.

```rust
use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

let processor_pool = Pool::builder()
    .worker_policy(WorkerPolicy::per_processor(nz!(2_u32)))
    .build();
let regional_pool = Pool::builder()
    .worker_policy(WorkerPolicy::per_memory_region(nz!(2_u32)))
    .build();
let system_pool = Pool::builder()
    .worker_policy(WorkerPolicy::per_system(nz!(2_u32)))
    .build();
```

A task is assigned using one processor observation during submission; regional routing
derives the `many_cpus` memory region from that same processor. Processor-policy workers
are pinned to the sampled processor. Regional workers share work from every submitting
processor in the selected region. System workers share work from every submitting processor
in the pool, treating the whole system as one locality group.

Each shared worker destination starts with an optional eager processor selection and accumulates
the processors that submitted accepted work. Without eager selection, first use starts workers
with the submitting processor's singleton affinity. A new submitting
processor expands the permitted set without adding workers. Repeated submissions from
known processors do not change the set. Processors are never removed.

Workers refresh their affinity at task boundaries; executing tasks may finish with the
previous narrower mask. Migration within the permitted mask is allowed even during a task,
but is not required. Regional work never moves to another region to obtain an idle worker.
System workers may move between permitted processors in different memory regions.

Each destination starts its configured worker count once, on first use or during eager
construction. Construction starts no workers by default. Workers persist until shutdown.
Shared worker counts are not multiplied
by the number of submitting processors; a system count is also independent of the number
of submitting regions. Counts must be nonzero but may exceed the
processor count, supporting blocking workloads.

## Eager construction

`PoolBuilder::eager` takes ownership of a nonempty `ProcessorSet` to move worker startup
and initialization into construction. It records the selection without starting workers.
Repeated calls replace the selection; `build()` uses the final policy, hardware, name
and initialization callback regardless of setter order.

The selection must belong to the pool's hardware instance. The pool validates the complete
selection before starting workers, including hardware identity rather than only numeric CPU IDs.
Construction leaves the calling thread's affinity unchanged.

Processor-local pools start the configured count for every selected processor, pinned individually.
Regional pools start the count for each distinct represented region, with affinity limited to
the selected processors in that region. System pools start their count once with the full selected
set as affinity. Shared workers may use selected processors before those processors submit work.
The pool respects the exact selection without independently applying a quota or widening regions.

A successful eager construction means every selected worker has applied affinity and completed
its initialization callback. The callback's effects are visible to the constructing thread.
This prepares workers, not task-specific storage, application caches or a guaranteed sleeping state.

Selection seeds membership rather than restricting future submissions. Submissions from other
eligible processors create missing destinations lazily or expand existing shared groups.
Workers and membership are never removed during the pool's lifetime.

## Worker thread names

`PoolBuilder::name` supplies an identifier included in worker thread names.
The naming format and the identifier's position within it are unspecified.

## Worker initialization

`PoolBuilder::thread_init` lets the pool owner configure thread-local state on each worker.
The optional function runs once per worker, after initial affinity setup and before that
worker executes any tasks. Building an unused lazy pool does not invoke it; eager construction
initializes every selected worker, including idle siblings. Repeated setters replace the function.

Initialization on different workers may overlap without a guaranteed order, and submission
does not wait for it. Eager construction waits for all selected workers to finish initialization.
Workers can therefore initialize independently, and an initializer may submit work to its
pool. Initializers must preserve affinity, must not wait for work submitted to the same
pool and must not shut down that pool. Eager initializers must not depend on construction
returning, including to publish a scheduler for their own pool. They run under the full initial
affinity mask of their destination. A panic is an infrastructure failure that stops and
poisons the pool, rather than a task failure.

## Locality boundaries

Every processor permitted by hard operating-system restrictions is a valid submission
destination. Processor-time quotas do not narrow destination eligibility: a caller outside
a quota-selected subset still requires local workers.

Shared workers stay within their initial selection plus the cumulative submitting-processor set.
An orchestrator using the same suitable stable quota-obeying set for eager selection and
submission therefore keeps worker placement within that set. Changing the orchestrator's
processors can grow accumulated membership. The pool
does not enforce a quota, shrink membership or clamp the worker count. Fractional quotas
and other process work can still cause throttling.

Affinity does not bind memory allocations, move existing data or guarantee cache hits.
Task code must preserve worker affinity. Dynamic topology and restriction refresh are not
supported. Platforms without processor-pinning support retain functional worker-pool
behavior, with a single fallback memory region but no physical locality guarantee.

Single-task latency on an otherwise idle pool remains the primary optimization target.

## Scheduling

All regular/urgent and joined/fire-and-forget submissions use the selected policy.
Regular and urgent work use separate priority classes. Workers prefer urgent work, but
tasks within a class may execute in any order and urgent work does not preempt a task that
is already running.

Tasks with join handles return `Result<R, TaskError<R>>` asynchronously. Successful execution
returns `Ok(value)`. `is_cancel()` identifies cancellation before the body was called;
`is_panic()` identifies a panic during execution. Task panics do not automatically unwind
the awaiting caller. Fire-and-forget tasks discard their output and report panics through tracing.

`into_body()` recovers a cancelled task as `impl FnOnce() -> R + Send + 'static`, preserving its
captures so callers can execute it directly or submit it elsewhere. `into_panic()` recovers
the original panic payload for inspection or resumption. Both return `Result<_, Self>`,
preserving the error when its outcome does not match the requested extraction.
Dropping a join handle does not cancel execution.

For error reporting without recovery, `into_diagnostic()` consumes the task error and releases
the original body or panic payload. It returns an opaque `Error + Send + Sync + 'static` that
retains the message, source chain, backtrace and enrichment. Recovery state itself remains
`Send`-only; converting to a diagnostic does not require synchronization or message flattening.

## Shutdown

Dropping the pool stops accepting work and waits for tasks that are already executing.
Queued tasks may be cancelled. Callers that require completion await the corresponding
join handles before dropping the pool. Cancellation completes joined handles even when
schedulers remain alive. Joined task captures remain available until their handle, error or
recovered body is dropped, or the recovered body executes. Fire-and-forget cancellation releases
captures during shutdown.
Submissions through retained schedulers after clean shutdown are immediately cancelled without
execution. Submission returns normally; a joined submission yields a recoverable cancellation.

## Infrastructure failures

Any infrastructure failure logs the problem, stops the entire pool and permanently
poisons it against further submissions. Queued tasks are cancelled and their joined
handles yield recoverable cancellation errors. Already executing synchronous tasks may finish;
the pool does not forcibly terminate threads. Every later submission panics synchronously,
even after the pool is dropped. Task panics retain their separate behavior and do not
poison the pool.

Worker startup establishes the full configured count and initial affinity before
accepting work for a destination. Eager construction failures, including initializer panics,
propagate on the constructing call after stopping and joining every created worker; no partial
pool is returned. User initialization side effects are not reversible, and the pool cannot
forcibly terminate a callback that does not return.
Failures detected during submission, including thread
creation and initial affinity setup, panic on the submitting call. A later affinity
refresh failure occurs on a worker, so the triggering submission may already have returned.
Dropping the pool joins every worker and propagates worker failures after cleanup.
