# vicinal implementation

User-visible behavior is defined in the package [design](design.md).

## Ownership and coordination

The public pool controls admission and terminal lifecycle; schedulers retain shared storage and
submit work through it. The selected policy controls routing plus group and thread creation.
Inside `pool_inner`, admission owns the task-transfer transaction, `PoolRuntime` coordinates
terminal state and worker participation, and the worker loop owns affinity refresh and waiting.
`WorkerCore` handles a single task iteration independently of thread management.

`WorkerGroups` owns an append-only collection of stable group allocations. `WorkerGroup` is a
cloneable Arc handle intended for transfer into a policy-created worker thread.
`WorkerGroupId` contains the registry identity and stable group address, so its selection and
resolution are copy-only. The established scheduler path neither clones nor upgrades an Arc;
reference-count changes happen only through explicit `WorkerGroups` or `WorkerGroup` cloning.
Group addition is serialized and cold. The registry never relocates or removes a live group.
The public APIs do not expose mutable queue or registry state.

The runtime separately retains a strong snapshot of each group state for terminal signaling
and failure cleanup. Group state does not retain the runtime, so these registrations cannot
form a strong cycle. Cleanup may clone registered group-state handles, but ordinary task
routing resolves IDs to registry-owned state without changing a reference count.

The default configuration uses two workers per processor, an internal tuning choice rather
than a numeric API promise. Thread names use `p` or `r` before the destination ID to identify
the policy compactly: `{pool_name}-p{processor_id}-{worker_index}` or
`{pool_name}-r{memory_region_id}-{worker_index}`. System workers use
`{pool_name}-s-{worker_index}` because they have no processor or region identity.
Default pool names include an incrementing pool counter. Only the inclusion of a caller's
chosen name is contractual; its position and the rest of the naming scheme are
implementation details.

The default policy maintains indexed routes to copyable IDs. A typed internal destination
distinguishes logical processors, memory regions and the system. Each submission samples one
processor and derives its regional destination from the same topology entry. Custom policies
may maintain any mapping, add groups during selection and retain cloned collections for later
asynchronous additions.

Each group owns its queues, wake and shutdown signals, task storage, result channels and
participation state. Workers consume only their group's queues.
Processor groups use direct indexed lookup. Regional groups use compact slots for the
populated regions, with binary search over sorted IDs; storage does not scale with the
highest region ID. Regions without eligible processors have no slot. System pools have a
single group, independent of processor and region counts. Default-policy lookups require neither a global map lock nor topology filtering.

## Task storage

Each worker group owns a heterogeneous object pool that routes task layouts to reusable
slots. Allocation is serialized because the pool facade is not shareable between threads.
The resulting owning handles can be queued, executed and released on worker threads
without holding or revisiting the allocation lock.

Task slot reservation happens while the allocation lock is held. Initialization and type
erasure happen after the lock is released so moving or dropping caller-owned task state
cannot poison shared allocator state. Workers obtain pinned access through the owning
handle, execute the task and then release the individual slot for reuse.

Task wrappers own execution and result delivery. Shared panic-disposal helpers isolate
user payload destruction from both task handling and pool cleanup.

Result channels use a separate typed pool because their layout and lifecycle do not
require heterogeneous task storage. Joined wrappers retain the original body and sender
together until execution consumes them. Dropping an unexecuted wrapper sends cancellation
with the original body, completing pending handles even while schedulers retain the registry.
Only cancellation boxes the body; normal execution uses the existing pooled task slot.
The recovery API hides that storage behind an opaque callable type so storage choices remain
internal to the implementation.
Recovering the owning heterogeneous slot would require moving ownership out of the queue's
handle rather than cancellation from the stored wrapper's destructor.

The public error owns its recovery state separately from its shared diagnostic source:
task captures and panic payloads need only be `Send`, not `Sync`. Error inspection exposes
no user state; extraction consumes the error. Task panic payloads retain their owning wrapper
with protected disposal, so dropping either channel endpoint or an unobserved panic error
cannot turn its payload destructor into an infrastructure failure. Extracting a panic transfers
disposal responsibility to the caller. Cancelled captures live in the result until recovery
or disposal, rather than being destroyed by queue draining while a handle remains alive.

Diagnostic conversion drops the recovery state and moves the existing `OhnoCore` into a private
payload-free error. This preserves diagnostic ownership without formatting it into a new message
or making recovery payloads `Sync`. The public conversion exposes only standard error bounds;
the executable examples enable application-error support through development dependencies.

## Queue and worker lifecycle

Normal and urgent queues are independent. A worker removes a task while holding only the
corresponding queue lock, then executes it after releasing the lock. User task code
therefore runs without any group-state lock held.
Each enqueue wakes an additional sleeping worker: notification coalescing must not leave
idle workers asleep while an already notified worker enters a blocking task.

Policy selection happens before queue admission. On the default policy's cold processor path,
the route's `OnceLock` serializes group creation, thread spawning and initial pin readiness.
That lock is absent from established routes. Shared-policy membership uses sorted atomic
membership entries so a known submitting processor takes no registry lock and builds no mask.

Default-policy pinning happens in each newly spawned thread before `execute_worker()`.
`execute_worker()` registers participation, runs initialization and then enters the worker
loop. Lazy startup waits for affinity readiness but not for `thread_init` completion. Eager
startup waits for initialization too. A custom policy owns its operating-system thread handles,
but every thread that reaches `execute_worker()` participates in the pool's shutdown barrier
and failure state.

### Worker initialization

The pool's shared configuration owns an optional boxed initialization callback. Workers
borrow it once before entering the task loop; task execution has no callback lookup or
additional allocation. The builder and schedulers do not expose the callback for invocation.
Its captures need not be unwind-safe because invocation happens only within the worker's
infrastructure-failure boundary, which stops and poisons the pool on panic.

Workers acknowledge successful initial affinity setup before invoking the callback.
The lazy submitter waits for affinity but not callback completion, so initializers may submit
work to the pool. No worker holds a pool-state lock while invoking the callback. Different
workers may initialize concurrently, and queued tasks remain untouched by each worker until
its own initialization returns. Shutdown waits for initializing workers as well as workers
executing tasks.

### Eager construction

The builder owns its optional processor selection and resolves final configuration only at build
time. It constructs the runtime and worker-group registry before calling policy `prepare()`.
If preparation unwinds, the runtime stops every registered group and waits for participating
workers before resuming the failure.

Default eager preparation validates both supplied processor handles and processors rebuilt through the
selection's backing hardware against the pool's eligible set. Full processor equality includes
hardware-instance identity; supplied handles alone do not establish the backend used for pinning.
It partitions the selection into
singletons for distinct processors, distinct regions or one system mask according to the policy.
Repeated entries never create additional worker groups. Group masks and shared
membership are seeded before spawning; no synthetic task is allocated, counted or executed.

Lazy and eager startup share `WorkerGroup::execute_worker()`. Eager preparation first seeds every
selected group's mask, then starts every selected group and waits for all callback acknowledgments.
Callbacks can overlap across groups. A callback panic records the first payload in `PoolRuntime`,
stops all groups and wakes the preparation waiter. The builder then retrieves the payload after
participating workers leave.

Workers enter the loop with their already-applied initial affinity revision. The first real
submission from a selected processor therefore needs neither startup nor membership expansion.
Unselected processors retain the ordinary lazy-startup and cumulative-expansion behavior.

### Affinity and failure coordination

Lazy default-policy workers start with a singleton selected from `SystemHardware::all_processors()`, retaining
hard operating-system restrictions independently of processor-time quotas. Eager workers start
with the selected destination mask. Processor groups
keep that affinity. Regional and system groups publish an immutable shared affinity snapshot and
revision when admitting work from a new CPU. A pool-wide sorted table pairs populated CPU IDs
with atomic membership flags because each CPU belongs to one fixed group within a pool.
Binary search keeps established submissions allocation-free and lock-free for membership
lookup, while table storage scales with eligible processors rather than the highest ID.
Processor-local pools do not allocate this table. Expansion selects initial members and already
admitted CPUs plus the current submitting processor from the destination. The system destination
admits eligible processors regardless of their memory region, reusing the same membership
and refresh protocol as regional groups.

Membership and snapshot publication happen during policy selection, before the task is
enqueued. Workers refresh after dequeueing but before invoking task code, so they
observe membership associated with that task. They also refresh between iterations and
recheck the revision after registering a sleep listener. Expansion wakes all idle workers
outside shared locks. Submissions from known processors neither construct a mask nor
broadcast a wakeup.

Each worker applies snapshots sequentially, outside the snapshot lock; no older revision
can overwrite a newer one on that worker. Running tasks retain their narrower mask until
the next boundary. Empty selections and pinning failures are explicit failures, not
permission to execute unpinned. A runtime refresh failure poisons the entire pool before
signaling every group and draining queued tasks outside locks. Participating workers are
tracked in the runtime's active count. The reporting worker never attempts to join itself;
pool destruction waits on the count and propagates the retained failure after cleanup.

The submission boundary retains caller-owned task state outside topology, result-channel
and allocation operations. It also catches poisoned rejection before destroying captures.
Wrapping transfers ownership into another protected preparation boundary; allocation consumes
the wrapper only after slot reservation succeeds. That boundary also retains the joined
receiver, so a cancelled body with panicking captures is disposed of outside the original unwind
when submission fails before returning a handle. Infrastructure failures stop admission
before separately destroying retained captures, so reentrant destructors cannot submit work.
The worker's dequeued-task boundary publishes the same pool-wide failure state before releasing
its task; worker entry then signals groups, drains queued work and propagates the original failure.
Task wrappers separately catch user-task panics, keeping them out of the
infrastructure boundary. Cleanup failures also poison the pool while cleanup continues.
Workers check the pool-wide stop flag before task execution, rather than waiting for the
group signal scan to reach their group.

Shutdown closes admission, signals every registered group, drains queues and waits for
participating workers. Enqueue checks the pool shutdown flag while holding its selected
queue lock. A group created by a retained scheduler after clean shutdown is terminal and
cannot start workers or retain work. Poisoned pools reject group creation entirely.
Draining pops tasks under the queue lock and destroys them outside it; reentrant callbacks
observe the terminal pool state. Cleanup continues after a destructor or worker panic,
propagating the first failure only after resources are released and participating workers exit.
Task preparation and rejected-startup cleanup catch the infrastructure failure before
destroying an owned task, so a panicking capture cannot cause a double panic during unwinding.
Suppressed panic payloads are also destroyed behind an unwind boundary. If that destruction
panics, its replacement payload is deliberately retained: recursively destroying arbitrary
payloads cannot provide bounded cleanup. This exceptional retention lets the remaining
tasks and participating workers be released while preserving the original failure.
Clean shutdown permits retained schedulers to cancel submissions normally. Poisoning
takes precedence over shutdown, permanently rejecting every submission with a panic.

### Mutation-test termination

Policy tests use real threads and explicit synchronization to establish execution, cancellation
and participation. Integration locality tests observe actual worker affinity, including
expanded shared masks. Self-submission tests transfer their only reporting sender into the
submitted task so missing submission disconnects the receiver.
Panic fixtures raise a safely droppable terminal replacement when their destructor fails:
missing containment must not cause unbounded recursive payload destruction.

The live-group and worker-participation terminal paths retain mutation exclusions for
uninterruptible sleeping states. Omitting one can strand a worker or queued capture, and
detecting that absence would require a timing-based failure. Normal shutdown, poisoning,
queue-draining and participation tests retain ordinary behavior coverage.

The native admission forwarder and task execution wrapper share their underlying coordination
exclusions. Removing either can omit an initializer or task-provided handshake. Cancellation
retains task captures for recovery, including senders, so blocked receivers cannot rely on
disconnection. Detecting cancellation alongside every lifecycle handshake would require a
separate completion-selection protocol throughout these fixtures. Ordinary tests retain real
thread startup, execution, cancellation recovery and cleanup coverage rather than replacing
those scenarios solely to mutate these coordination boundaries.

## Initialization benchmarks

The Criterion `vicinal/initialization` group compares lazy and eager startup separately from
steady-state dispatch. Both modes use the same fixed processor-local worker count and a caller
pinned to the eager selection's single processor. This keeps the first submission on an already
selected destination rather than measuring lazy expansion of an eager pool. No application
initialization callback runs, so the comparison isolates the library's startup costs.

The `create` cases time `PoolBuilder::build()`, including eager worker readiness. Builder
configuration and processor-selection cloning happen outside measurement. The `first_task` cases
build a fresh pool and acquire its scheduler outside measurement, then time regular submission
through successful join-handle completion using the same small CPU-only task as the dispatch
benchmarks. No task has previously run on that pool, so both modes include first-use task storage
and result-channel allocation; lazy submission also includes worker startup.

Per-iteration batching keeps pool teardown and worker waits outside both measurements and finishes
them before preparing another pool. Criterion warmup also uses fresh pools rather than warming
the pool that a later iteration measures. These wall-clock comparisons include OS scheduling and
worker creation, so `.cargo/bench_history.toml` excludes the entire group from automated analysis
while retaining results for manual inspection. They have no Callgrind counterpart.

## Logging coverage

Pool lifecycle events use the pool implementation's tracing target, and task events use the
task wrapper's target, including events emitted by their internal coordination and cleanup
modules. Log filtering therefore follows the owning component rather than its helper layout.

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
and pool destruction waits for workers before output inspection. These scenarios exercise the
production failure boundaries without relying on scheduling delays or exposing internals.
