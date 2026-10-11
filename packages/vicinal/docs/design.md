# vicinal design

`vicinal` executes synchronous tasks near the thread that submitted them. Callers choose
whether locality means the same logical processor or the same memory region.

## Worker policies

`WorkerPolicy` is an opaque configuration value. Constructors select a fixed worker count
per processor or per memory region, without a policy trait or generic pool and scheduler
types. Additional constructors can extend configuration without exposing its representation.

The default is two workers per processor. `workers_per_processor(count)` is a convenience
alias for selecting the corresponding policy. Both setters replace the same policy, so the
last setter wins. A built pool's policy is immutable.

```rust
use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

let processor_pool = Pool::builder().workers_per_processor(nz!(2_u32)).build();
let regional_pool = Pool::builder()
    .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
    .build();
```

A task is assigned to the current processor or `many_cpus` memory region sampled once
during submission. Processor-policy workers are pinned to that processor. Regional workers
share work from every submitting processor in the selected region and may float among
that region's eligible processors, even during one task. Running on the submitting
processor remains valid; migration is permitted, not required. Work never moves to another
region to obtain an idle worker.

Each destination starts its configured worker count once, on first use. Construction
starts no workers. Workers persist until shutdown, and a regional count is not multiplied
by the number of submitting processors. Counts must be nonzero but may exceed the
processor count, supporting blocking workloads.

## Locality boundaries

Every processor permitted by hard operating-system restrictions is eligible for workers.
Processor-time quotas do not narrow this set: a caller outside a quota-selected subset
still requires local workers. The fixed worker count does not guarantee avoidance of
processor-time quota throttling.

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

Dropping the pool signals its workers and waits for tasks that are already executing.
Queued tasks may be abandoned. Callers that require completion await the corresponding
join handles before dropping the pool. Abandonment releases captured resources and
disconnects result handles even when schedulers retain the pool's storage.

Worker startup must establish the full configured count and affinity before accepting
work for a destination. Thread creation or affinity failure is reported to the submitting
caller rather than permitting unpinned execution or work with no functioning workers.
