# vicinal implementation

User-visible behavior is defined in the package [design](design.md).

The default configuration uses two workers per processor, an internal tuning choice rather
than a numeric API promise. Thread names use `p` or `r` before the destination ID to identify
the policy compactly: `{pool_name}-p{processor_id}-{worker_index}` or
`{pool_name}-r{memory_region_id}-{worker_index}`. System workers use
`{pool_name}-s-{worker_index}` because they have no processor or region identity.
Default pool names include an incrementing pool counter. Only the inclusion of a caller's
chosen name is contractual; its position and the rest of the naming scheme are
implementation details.

The pool maintains an indexed registry of worker groups created on demand. A typed
destination distinguishes logical processors, memory regions and the system. Each submission
samples one processor and derives its regional destination from the same cached topology entry.
This coherent target is reused for startup, allocation, queueing and diagnostics.

Each group owns its queues, wake and shutdown signals, task storage, result channels and
startup outcome. Workers consume only their group's queues.
Processor groups use direct indexed lookup. Regional groups use compact slots for the
populated regions, with binary search over sorted IDs; storage does not scale with the
highest region ID. Regions without eligible processors have no slot. System pools have a
single lazy group, independent of processor and region counts. Initialized lookups
require neither a global map lock nor topology filtering.

## Task storage

Each worker group owns a heterogeneous object pool that routes task layouts to reusable
slots. Allocation is serialized because the pool facade is not shareable between threads.
The resulting owning handles can be queued, executed and released on worker threads
without holding or revisiting the allocation lock.

Task slot reservation happens while the allocation lock is held. Initialization and type
erasure happen after the lock is released so moving or dropping caller-owned task state
cannot poison shared allocator state. Workers obtain pinned access through the owning
handle, execute the task and then release the individual slot for reuse.

## Queue and worker lifecycle

Normal and urgent queues are independent. A worker removes a task while holding only the
corresponding queue lock, then executes it after releasing the lock. User task code
therefore runs without any group-state lock held.
Each enqueue wakes an additional sleeping worker: notification coalescing must not leave
idle workers asleep while an already notified worker enters a blocking task.

Cold startup, shared worker membership expansion and handle registration share a pool-level lock
with the shutdown transition. This lock is absent from submissions made on known processors.
Task allocation precedes the cold transaction, so another processor cannot overtake the
initial submission between worker startup and admission. Startup publishes its outcome only
after every named worker acknowledges successful affinity setup. Partial thread creation
or pinning failure marks the pool poisoned, stops every group and propagates
the panic outside the lock. All created handles remain accounted for until joined; failed
pools cannot accept work or silently retry.

Workers start with a singleton selected from `SystemHardware::all_processors()`, retaining
hard operating-system restrictions independently of processor-time quotas. Processor groups
keep that affinity. Regional and system groups publish an immutable shared affinity snapshot and
revision when admitting work from a new CPU. A pool-wide sorted table pairs populated CPU IDs
with atomic membership flags because each CPU belongs to one fixed group within a pool.
Binary search keeps established submissions allocation-free and lock-free for membership
lookup, while table storage scales with eligible processors rather than the highest ID.
Processor-local pools do not allocate this table. Expansion selects only already admitted
CPUs plus the current submitting processor from the destination. The system destination
admits eligible processors regardless
of their memory region, reusing the same membership and refresh protocol as regional groups.

Queue insertion, membership and snapshot publication happen under the selected queue's
admission lock. Workers refresh after dequeueing but before invoking task code, so they
observe membership associated with that task. They also refresh between iterations and
recheck the revision after registering a sleep listener. Expansion wakes all idle workers
outside shared locks. Submissions from known processors neither construct a mask nor
broadcast a wakeup.

Wait-boundary tests observe workers after listener registration and the final queue/revision
recheck, immediately before the unchanged listener wait. A group-local test-only callback
holds those registered wait attempts while the test expands affinity. Initial admission
finishes before these workers start, so earlier notifications cannot satisfy the waits.

Each worker applies snapshots sequentially, outside the snapshot lock; no older revision
can overwrite a newer one on that worker. Running tasks retain their narrower mask until
the next boundary. Empty selections and pinning failures are explicit failures, not
permission to execute unpinned. A runtime refresh failure poisons the entire pool before
signaling every group and draining queued tasks outside locks. All worker handles remain
registered: the reporting worker must not attempt to join itself. Pool destruction joins
every handle and propagates worker failures after cleanup.

The submission boundary retains caller-owned task state outside topology, result-channel
and allocation operations. It also catches poisoned rejection before destroying captures.
Wrapping transfers ownership into another protected preparation boundary; allocation consumes
the wrapper only after slot reservation succeeds. Infrastructure failures stop admission
before separately destroying retained captures, so reentrant destructors cannot submit work.
The worker's dequeued-task boundary publishes the same pool-wide failure state before releasing
its task; worker entry then signals groups, drains queued work and propagates the original failure.
Task wrappers separately catch user-task panics, keeping them out of the
infrastructure boundary. Cleanup failures also poison the pool while cleanup continues.
Workers check the pool-wide stop flag before task execution, rather than waiting for the
group signal scan to reach their group.

Shutdown closes admission under the startup lock, signals groups, drains queues and joins
every registered handle. Enqueue checks the pool shutdown flag while holding its selected
queue lock. A group created after the shutdown scan cannot start workers or retain work.
Draining pops tasks under the queue lock and destroys them outside it; reentrant callbacks
observe the terminal pool state. Cleanup continues after a destructor or worker panic,
propagating the first failure only after resources are released and handles joined.
Task preparation and rejected-startup cleanup catch the infrastructure failure before
destroying an owned task, so a panicking capture cannot cause a double panic during unwinding.
Suppressed panic payloads are also destroyed behind an unwind boundary. If that destruction
panics, its replacement payload is deliberately retained: recursively destroying arbitrary
payloads cannot provide bounded cleanup. This exceptional retention lets the remaining
tasks and worker handles be released while preserving the original failure.
Clean shutdown permits retained schedulers to abandon submissions normally. Poisoning
takes precedence over shutdown, permanently rejecting every submission with a panic.

Result channels use a separate typed pool because their layout and lifecycle do not
require heterogeneous task storage. Disconnecting abandoned senders completes pending
join handles even while scheduler clones retain the registry. Task panic payloads have an
owning wrapper with protected disposal. Dropping either result-channel endpoint therefore
cannot turn an unobserved task panic's destructor into an infrastructure failure; awaiting
the handle extracts and resumes the original payload.

## Logging coverage

Ordinary unit and integration test executables initialize permanent silent formatting
with `testing::init_tracing!()`. Existing private failure adapters can therefore exercise
logging expressions and formatters without adding public test APIs. Tests share the
enabled subscriber configuration without capturing or mixing their output.

The dedicated `tracing_coverage` integration binary instead owns a capturing subscriber
for its single test, allowing assertions about events emitted across worker threads.
Its scenarios use the public pool and scheduler APIs with simulated hardware, including
both locality policies, priorities, task panic payloads, retained schedulers and shutdown
during an outer unwind.

Failure-path logging needs no private failure adapter: dropping a disconnected task result
can fail on its worker, poisoned rejection can encounter a panicking capture, and queued
capture destruction can fail during shutdown. A blocked worker keeps those captures queued;
another queued capture releases it during cleanup. Explicit signals establish execution
and pool destruction joins workers before output inspection. These scenarios exercise the
production failure boundaries without relying on scheduling delays or exposing internals.
