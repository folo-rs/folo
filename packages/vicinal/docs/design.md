# vicinal design

`vicinal` executes synchronous tasks on worker threads. A caller submits a task by calling
one of a pool's `Scheduler::spawn*` methods. The pool samples the calling thread's current
logical processor during that call; this is the task's submitting processor.

Callers choose whether locality means the same logical processor, the same memory region
or the cumulative set of submitting processors across the system. Processor affinity is
the set of processors on which a worker is permitted to run.

## Worker policies

`WorkerPolicy` is an opaque configuration value. Constructors select a fixed worker count
per processor, per memory region or per system, without a policy trait or generic pool and
scheduler types. Counts and processor membership are independent for each pool.
Additional constructors can extend configuration without exposing its representation.

The default is processor-local scheduling. Applications configure explicit counts through
`WorkerPolicy`. The legacy `workers_per_processor(count)` convenience alias remains callable
but is deprecated and hidden from public documentation. Its deprecation directs callers to
`worker_policy(WorkerPolicy::fixed_per_processor(count))`. Both setters replace the same policy,
so the last setter wins. A built pool's policy is immutable.

```rust
use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

let processor_pool = Pool::builder()
    .worker_policy(WorkerPolicy::fixed_per_processor(nz!(2_u32)))
    .build();
let regional_pool = Pool::builder()
    .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
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

Each shared worker destination accumulates the processors that submitted accepted work. First use
starts workers with the submitting processor's singleton affinity. A new submitting
processor expands the permitted set without adding workers. Repeated submissions from
known processors do not change the set. Processors are never removed.

Workers refresh their affinity at task boundaries; executing tasks may finish with the
previous narrower mask. Migration within the permitted mask is allowed even during a task,
but is not required. Regional work never moves to another region to obtain an idle worker.
System workers may move between submitting processors in different memory regions.

Each destination starts its configured worker count once, on first use. Construction
starts no workers. Workers persist until shutdown. Shared worker counts are not multiplied
by the number of submitting processors; a system count is also independent of the number
of submitting regions. Counts must be nonzero but may exceed the
processor count, supporting blocking workloads.

## Worker initialization

`PoolBuilder::thread_init` lets the pool owner configure thread-local state on each worker.
The optional function runs once per worker, after initial affinity setup and before that
worker executes any tasks. Building an unused pool does not invoke it. Repeated setters
replace the function.

Initialization on different workers may overlap, and submission does not wait for it.
Workers can therefore initialize independently, and an initializer may submit work to its
pool. Initializers must preserve affinity, must not wait for work submitted to the same
pool and must not shut down that pool. A panic is an infrastructure failure that stops and
poisons the pool, rather than a task failure.

## Locality boundaries

Every processor permitted by hard operating-system restrictions is a valid submission
destination. Processor-time quotas do not narrow destination eligibility: a caller outside
a quota-selected subset still requires local workers.

Shared workers stay within the cumulative submitting-processor set. An orchestrator
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

Tasks with join handles return their output asynchronously. A task panic is captured and
resumed when the handle is awaited. Fire-and-forget tasks discard their output and report
panics through tracing.

## Shutdown

Dropping the pool stops accepting work and waits for tasks that are already executing.
Queued tasks may be abandoned. Callers that require completion await the corresponding
join handles before dropping the pool. Abandonment releases captured resources and
completes abandoned join handles with a panic even when schedulers remain alive.
Submissions through retained schedulers after clean shutdown are immediately abandoned without
execution. Submission returns normally; awaiting an abandoned handle panics.

## Infrastructure failures

Any infrastructure failure logs the problem, stops the entire pool and permanently
poisons it against further submissions. Queued tasks are abandoned and their joined
handles panic when awaited. Already executing synchronous tasks may finish;
the pool does not forcibly terminate threads. Every later submission panics synchronously,
even after the pool is dropped. Task panics retain their separate behavior and do not
poison the pool.

Worker startup establishes the full configured count and singleton affinity before
accepting work for a destination. Failures detected during submission, including thread
creation and initial affinity setup, panic on the submitting call. A later affinity
refresh failure occurs on a worker, so the triggering submission may already have returned.
Dropping the pool joins every worker and propagates worker failures after cleanup.
