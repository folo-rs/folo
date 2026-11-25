# vicinal design

`vicinal` executes synchronous tasks on worker threads. A caller submits a task by calling
one of a pool's `Scheduler::spawn*` methods. The pool samples the calling thread's current
logical processor during that call; this is the task's submitting processor.

Callers choose whether locality means the same logical processor, the same memory region
or a shared set of processors across the system. Processor affinity is
the set of processors on which a worker is permitted to run.

## Worker policies

`WorkerPolicy` is the interface for assigning tasks to groups and for deciding when groups
and workers are created. The `policies` module supplies processor, memory-region and system
implementations. The pool does not expose a generic type parameter for either its policy or
scheduler, so changing policies does not change the rest of the application API.

The default is processor-local scheduling. Applications configure explicit counts through
`WorkersPerProcessor`, `WorkersPerMemoryRegion` or `WorkersPerSystem` and the builder's
`worker_policy(...)` setter. Custom policy values also use this setter. Each call replaces
the selected policy, so the last setter wins. A built pool's policy is immutable.

```rust
use new_zealand::nz;
use vicinal::policies::{
    WorkersPerMemoryRegion, WorkersPerProcessor, WorkersPerSystem,
};
use vicinal::Pool;

let processor_pool = Pool::builder()
    .worker_policy(WorkersPerProcessor::new(nz!(2_u32)))
    .build();
let regional_pool = Pool::builder()
    .worker_policy(WorkersPerMemoryRegion::new(nz!(2_u32)))
    .build();
let system_pool = Pool::builder()
    .worker_policy(WorkersPerSystem::new(nz!(2_u32)))
    .build();
```

The pool calls the selected policy's `prepare()` once with the final hardware, eager selection,
worker groups and name. `select_group()` runs for every task submission and returns a copyable
`WorkerGroupId`. The scheduler resolves that ID by borrowing the group; established task
submission neither clones `WorkerGroups` nor `WorkerGroup`.

A policy may add groups during preparation or selection and may clone a `WorkerGroups`
handle to add groups later. To add a worker, the policy calls `WorkerGroup::add_worker()`,
moves the returned `Worker` into the new thread, applies the intended affinity and calls
the blocking `Worker::run()` method. Vicinal then owns task execution, worker initialization,
failure containment and worker lifetime through shutdown.

Every `Worker` created while `prepare()` runs creates an individual preparation-time
initialization reservation. After `prepare()` returns, the builder waits until each exact
reserved `Worker` runs its initialization callback. A policy that cannot run one reserved
worker drops that specific value to cancel only its reservation. Retaining a reserved worker
without running or dropping it leaves pool construction waiting. Workers created after
`prepare()` returns do not participate in this wait, even if construction is still waiting
for earlier reservations.

A policy may retain a `WorkerGroup` handle and inspect its participating worker count with
`worker_count()` or its combined urgent/regular backlog with `queue_len()`. These are instantaneous
observations rather than reservations. Selection runs before the incoming task is admitted,
so `queue_len()` does not include that task. The stand-alone `vicinal_growing_pool` example
uses this boundary to add a worker whenever a later task submission observes existing backlog.
It starts with one worker and grows without a configured maximum. Once a worker enters
`Worker::run()`, the API keeps it alive until the pool shuts down; policies cannot remove it.

The built-in policies assign a task using one processor observation during task submission;
regional routing derives the `many_cpus` memory region from that same processor.
Processor-policy workers are pinned to the sampled processor. Regional workers share work
from every submitting processor in the selected region. System workers share work from every
submitting processor in the pool, treating the whole system as one locality group.

Each shared worker group starts with an optional eager processor selection and accumulates
processors when their task submissions reach the built-in policy selection step. Without eager
selection, first use starts workers permitted to run only on the submitting processor. A new
submitting processor expands the permitted set without adding workers. Repeated task submissions
from known processors do not change the set. The policy cannot remove processors from that set.

Workers refresh their affinity at task boundaries; executing tasks may finish with the
previous narrower mask. Migration within the permitted mask is allowed even during a task,
but is not required. Regional work never moves to another region to obtain an idle worker.
System workers may move between permitted processors in different memory regions.

Each built-in worker group starts its configured worker count once, on first task use or
during eager construction. The default policy starts no workers during construction
unless eager selection is configured. Every policy receives the eager selection during
`prepare()` through `WorkerPolicyContext::eager()`. Built-in policies honor it automatically;
a custom policy may interpret or ignore it, but the builder still waits for any workers that
the policy reserves with `WorkerGroup::add_worker()` during `prepare()`. Workers persist until
shutdown. Shared worker counts are not multiplied
by the number of submitting processors; a system count is also independent of the number
of submitting regions. Counts must be nonzero but may exceed the
processor count, supporting blocking workloads.

## Eager construction

`PoolBuilder::eager` takes ownership of a nonempty `ProcessorSet` and passes it to the
selected policy during `prepare()`. The setter records the selection without starting workers.
Repeated calls replace the selection; `build()` uses the final policy, hardware, name
and initialization callback regardless of setter order. A custom policy decides how the
eager selection maps to worker groups. The policy reserves each eager worker with
`WorkerGroup::add_worker()` during `prepare()`; the builder waits for those workers after
the policy returns.

The selection must belong to the pool's hardware instance. The pool validates the complete
selection before starting workers, including hardware identity rather than only numeric CPU IDs.
Construction leaves the calling thread's affinity unchanged.

Processor-local pools start the configured count for every selected processor, pinned individually.
Regional pools start the count for each distinct represented region, with affinity limited to
the selected processors in that region. System pools start their count once with the full selected
set as affinity. Shared workers may use selected processors before those processors submit work.
The pool respects the exact selection without independently applying a quota or widening regions.

A successful eager construction means every worker required by the policy has applied
affinity and completed its initialization callback. The callback's effects are visible
to the constructing thread.
This prepares workers, not task-specific storage, application caches or a guaranteed sleeping state.

Selection seeds membership rather than restricting future task submissions. Task submissions
from other eligible processors create missing worker groups lazily or expand existing shared
groups. Neither the worker group API nor the built-in policies remove running workers or
previously admitted processor membership during the pool's lifetime.

## Worker thread names

`PoolBuilder::name` supplies an identifier included in worker thread names.
The naming format and the identifier's position within it are unspecified.

## Worker initialization

`PoolBuilder::thread_init` lets the pool owner configure thread-local state on each worker.
The optional function runs once per worker, after initial affinity setup and before that
worker executes any tasks. Building an unused lazy pool does not invoke it; eager construction
initializes every selected worker, including idle siblings. Repeated setters replace the function.

Initialization on different workers may overlap without a guaranteed order, and task submission
does not wait for it. Eager construction waits for all selected workers to finish initialization.
Workers can therefore initialize independently, and an initializer may submit work to its
pool. Initializers must preserve affinity, must not wait for work submitted to the same
pool and must not shut down that pool. Eager initializers must not depend on construction
returning, including to publish a scheduler for their own pool. They run under the full initial
affinity mask of their destination. A panic is an infrastructure failure that stops and
poisons the pool, rather than a task failure.

## Locality boundaries

Every processor permitted by hard operating-system restrictions can submit tasks to the pool.
Processor-time quotas do not narrow that eligibility: a caller outside a quota-selected subset
still requires local workers.

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

Dropping the pool stops accepting work and waits for every running worker to leave.
Queued tasks may be cancelled. Callers that require completion await the corresponding
join handles before dropping the pool. Cancellation completes joined handles even when
schedulers remain alive. Joined task captures remain available until their handle, error or
recovered body is dropped, or the recovered body executes. Fire-and-forget cancellation releases
captures during shutdown.
Task submissions through retained schedulers after clean shutdown are immediately cancelled
without execution. A submission returns normally; a joined task submission yields a recoverable
cancellation. A worker callback must not synchronously drop its own pool because shutdown
waits for that worker to return from the callback.

## Infrastructure failures

Any infrastructure failure logs the problem, stops the entire pool and permanently
poisons it against further submissions. Queued tasks are cancelled and their joined
handles yield recoverable cancellation errors. Already executing synchronous tasks may finish;
the pool does not forcibly terminate threads. Every later submission panics synchronously,
even after the pool is dropped. Task panics retain their separate behavior and do not
poison the pool.

Built-in policy startup establishes the full configured count and initial affinity before
accepting work for a worker group. Eager construction failures, including initializer panics,
propagate on the constructing call after stopping and waiting for every running worker;
no partial pool is returned. User initialization side effects are not reversible, and the
pool cannot forcibly terminate a callback that does not return.
Failures detected during task submission, including thread
creation and initial affinity setup, panic on the submitting call. A later affinity
refresh failure occurs on a worker, so the triggering submission may already have returned.
Dropping the pool waits for every running worker and propagates worker failures
after cleanup. A custom policy owns the operating-system thread handles, if it needs them,
while `Worker::run()` supplies the pool's worker-lifetime and failure boundaries.
