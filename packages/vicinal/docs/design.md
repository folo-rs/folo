# vicinal design

`vicinal` executes synchronous tasks on worker threads. A caller submits a task by calling
one of a pool's `Scheduler::spawn*` methods. The pool samples the calling thread's current
logical processor during that call; this is the task's submitting processor.

Callers choose whether locality means the same logical processor, the same memory region
or a shared set of processors across the system. Processor affinity is
the set of processors on which a worker is permitted to run.

## Worker policies

A worker group is a task destination with its own queues; its participating workers consume
only those queues.

`vicinal::worker_policy::WorkerPolicy` assigns tasks to worker groups and decides when groups
and workers are created. The `vicinal::policies` module supplies processor,
memory-region and system implementations. The pool does not expose a generic type parameter for
either its policy or scheduler, so changing policies does not change the rest of the application
API.

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
    .build().unwrap();
let regional_pool = Pool::builder()
    .worker_policy(WorkersPerMemoryRegion::new(nz!(2_u32)))
    .build().unwrap();
let system_pool = Pool::builder()
    .worker_policy(WorkersPerSystem::new(nz!(2_u32)))
    .build().unwrap();
```

The pool calls `bind()` once, consuming the configured `WorkerPolicy` and passing the final
pool settings in a `BindContext`. The call returns the `BoundPolicy` shared by schedulers.
For each submission that reaches routing, `BoundPolicy::select_group()` returns a
`WorkerGroupId` belonging to the pool. Selection does not guarantee admission.

A policy may add groups during binding or selection and may clone a `WorkerGroups`
handle to add groups later. To add a worker, the policy calls `WorkerGroup::add_worker()`,
moves the returned `Worker` into the new thread, applies the intended affinity and calls
the blocking `Worker::run()` method. Vicinal then owns task execution, worker initialization,
failure containment and worker lifetime through shutdown.

Binding-time worker reservations and their readiness boundary follow the rules in
[Eager construction](#eager-construction).

A policy may retain a `WorkerGroup` handle and inspect its participating worker count with
`worker_count()` or its combined urgent/regular backlog with `queue_len()`. These are instantaneous
observations rather than reservations. Selection runs before the incoming task is admitted,
so `queue_len()` does not include that task. The stand-alone `vicinal_growing_pool` example
uses this boundary to attempt growth whenever a later task submission observes existing backlog.
It requires one worker at binding time and leaves existing workers to drain the queue if later
growth fails. It has no configured maximum. Once a worker enters
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
previous narrower processor set. Migration within the permitted set is allowed even during a task,
but is not required. Regional work never moves to another region to obtain an idle worker.
System workers may move between permitted processors in different memory regions.

Each built-in worker group starts its configured worker count once, on first task use or
during eager construction. The default policy starts no workers during construction
unless eager selection is configured. Every policy receives the eager selection during
`bind()` through `BindContext::eager()`. Built-in policies honor it automatically;
a custom policy may interpret or ignore it. Workers persist until shutdown. Shared worker
counts are not multiplied by the number of submitting processors; a system count is also
independent of the number of submitting regions. Counts must be nonzero but may exceed the
processor count, supporting blocking workloads.

## Eager construction

`PoolBuilder::eager` takes ownership of a nonempty `ProcessorSet` without starting workers.
`build()` passes the selection to the selected policy during `bind()`.
Repeated calls replace the selection; `build()` uses the final policy, hardware, name and
initialization callback regardless of setter order. A custom policy decides how the eager
selection maps to worker groups and may ignore it.

Regardless of eager selection, the builder waits after `bind()` returns for workers reserved
during binding to finish initialization or for the policy to cancel their reservations by
dropping the unrun workers. The policy must run or drop those workers without waiting for
`build()` to return. Workers reserved after `bind()` returns are not part of this wait.

The selection must belong to the pool's hardware instance. The pool validates the complete
selection before starting workers, including hardware identity rather than only numeric CPU IDs.
A mismatch returns `BuildError` without invoking the selected policy. Construction leaves the
calling thread's affinity unchanged.

Processor-local pools start the configured count for every selected processor, pinned individually.
Regional pools start the count for each distinct represented region, with affinity limited to
the selected processors in that region. System pools start their count once with the full selected
set as affinity. Shared workers may use selected processors before those processors submit work.
The pool respects the exact selection without independently applying a quota or widening regions.

With a built-in policy, successful eager construction means every worker required by the
eager selection has applied affinity and completed its initialization callback. The callback's
effects are visible to the constructing thread. This prepares workers, not task-specific
storage, application caches or a guaranteed sleeping state.

Selection seeds membership rather than restricting future task submissions. Task submissions
from other eligible processors create missing worker groups lazily or expand existing shared
groups. The worker group API and built-in policies never remove individual workers, and
built-in shared groups never remove processors from the set on which their workers may run.

## Worker thread names

`PoolBuilder::name` supplies an identifier included in thread names created by built-in policies.
The naming format and the identifier's position within it are unspecified. Custom policies can
read the identifier with `BindContext::pool_name()` when naming their own threads.

## Worker initialization

`PoolBuilder::thread_init` lets the pool owner configure thread-local state on each worker.
The optional function runs once per worker, after initial affinity setup and before that
worker executes any tasks. Built-in policies do not invoke it when building a lazy pool;
with eager selection, they initialize every selected worker. Repeated setters replace the function.

Initialization on different workers may overlap without a guaranteed order, and task submission
does not wait for it. Construction waits for workers reserved during binding to finish initialization.
Workers can therefore initialize independently, and an initializer may submit work to its
pool. Initializers must preserve affinity, must not wait for work submitted to the same
pool and must not shut down that pool. Initializers for binding-time workers must not depend on
construction returning, including to publish a scheduler for their own pool. They run with the worker
group's full initial permitted processor set. A panic is an infrastructure failure that stops and
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

All regular/urgent and joined/fire-and-forget submissions that reach worker-group routing use
the selected policy. A submission that detects clean shutdown before routing bypasses the
policy; selection does not guarantee acceptance if shutdown starts before queue admission.
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

Dropping the pool stops accepting work and waits for participating workers to leave `Worker::run()`.
It does not join policy-owned operating-system threads.
Queued tasks may be cancelled. Callers that require completion await the corresponding
join handles before dropping the pool. Cancellation completes joined handles even when
schedulers remain alive. Joined task captures remain available until their handle, error or
recovered body is dropped, or the recovered body executes. Fire-and-forget cancellation releases
captures during shutdown.
Task submissions through retained schedulers after clean shutdown are immediately cancelled
without execution. A submission returns normally; a joined task submission yields a recoverable
cancellation. Keep a pool owner until required work finishes and do not release the last
owner from one of its worker callbacks. Capture a scheduler for follow-up submissions.

## Infrastructure failures

Any infrastructure failure logs the problem, stops the entire pool and permanently
poisons it against further submissions. Queued tasks are cancelled and their joined
handles yield recoverable cancellation errors. Already executing synchronous tasks may finish;
the pool does not forcibly terminate threads. Every later submission panics synchronously,
even after the pool is dropped. Task panics retain their separate behavior and do not
poison the pool.

Built-in policy startup establishes every configured worker for a worker group and applies its
initial affinity before accepting work for that group. On ordinary startup failures during
construction, including initializer panics, `build()` stops the pool, waits for all participating
workers to leave `Worker::run()` and returns a `BuildError` without returning a partial pool.
If the operating system refuses to create an eager worker thread for a built-in policy, the
error retains the destination and the original I/O error in its source chain. Incidental
panics, for example when dropping rejected custom policy state, may still unwind.
User initialization side effects are not reversible, and the pool cannot forcibly terminate
a callback that does not return.
Built-in startup failures detected during task submission, including thread creation and initial
affinity setup, panic on the submitting call. A failure while a worker applies a later permitted
affinity set can occur after task submission has already returned.
Dropping the pool propagates worker failures after cleanup.
