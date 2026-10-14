# vicinal design

`vicinal` executes synchronous tasks near the thread that submitted them. Callers choose
whether locality means the same logical processor or the same memory region.

## Worker policies

`WorkerPolicy` is an opaque configuration value. Constructors select a fixed worker count
per processor or per memory region, without a policy trait or generic pool and scheduler
types. Additional constructors can extend configuration without exposing its representation.

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
```

A task is assigned using one processor observation during submission; regional routing
derives the `many_cpus` memory region from that same processor. Processor-policy workers
are pinned to the sampled processor. Regional workers share work from every submitting
processor in the selected region.

Each pool and region accumulates the processors that submitted accepted work. First use
starts workers with the submitting processor's singleton affinity. A new submitting
processor expands the permitted set without adding workers. Repeated submissions from
known processors do not change the set. Processors are never removed.

Workers refresh their affinity at task boundaries; executing tasks may finish with the
previous narrower mask. Migration within the permitted mask is allowed even during a task,
but is not required. Work never moves to another region to obtain an idle worker.

Each destination starts its configured worker count once, on first use. Construction
starts no workers. Workers persist until shutdown, and a regional count is not multiplied
by the number of submitting processors. Counts must be nonzero but may exceed the
processor count, supporting blocking workloads.

## Locality boundaries

Every processor permitted by hard operating-system restrictions is a valid submission
destination. Processor-time quotas do not narrow destination eligibility: a caller outside
a quota-selected subset still requires local workers.

Regional workers stay within the cumulative submitting-processor set. An orchestrator
using a suitable stable quota-obeying set therefore keeps worker placement within that
set. Changing the orchestrator's processors can grow accumulated membership. The pool
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

Worker startup establishes the full configured count and singleton affinity before
accepting work for a destination. Failures detected during submission, including thread
creation and initial affinity setup, panic on the submitting call. A later affinity
refresh failure occurs on a worker, so the triggering submission may already have returned.
Dropping the pool joins every worker and propagates worker failures after cleanup.
