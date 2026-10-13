# vicinal implementation

User-visible behavior is defined in the package [design](design.md).

The default configuration uses two workers per processor, an internal tuning choice rather
than a numeric API promise. Thread names use `p` or `r` before the destination ID to identify
the policy compactly: `{pool_name}-p{processor_id}-{worker_index}` or
`{pool_name}-r{memory_region_id}-{worker_index}`. Default pool names include an incrementing
pool counter. The naming scheme is diagnostic information, not a stable API contract.

The pool maintains an indexed registry of worker groups created on demand. A typed
destination distinguishes logical processors from memory regions. Each submission samples
one processor and derives its regional destination from the same cached topology entry.
This coherent target is reused for startup, allocation, queueing and diagnostics.

Each group owns its queues, wake and shutdown signals, task storage, result channels and
startup outcome. Workers consume only their group's queues.
Processor groups use direct indexed lookup. Regional groups use compact slots for the
populated regions, with binary search over sorted IDs; storage does not scale with the
highest region ID. Regions without eligible processors have no slot. Initialized lookups
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

Cold startup, regional membership expansion and handle registration share a pool-level lock
with the shutdown transition. This lock is absent from established submitters' submissions.
Task allocation precedes the cold transaction, so another processor cannot overtake the
initial submission between worker startup and admission. Startup publishes its outcome only
after every named worker acknowledges successful affinity setup. Partial thread creation
or pinning failure marks the pool poisoned, stops every group and propagates
the panic outside the lock. All created handles remain accounted for until joined; failed
pools cannot accept work or silently retry.

Workers start with a singleton selected from `SystemHardware::all_processors()`, retaining
hard operating-system restrictions independently of processor-time quotas. Processor groups
keep that affinity. Regional groups publish an immutable shared affinity snapshot and
revision when admitting work from a new CPU. A pool-wide sorted table pairs populated CPU IDs
with atomic membership flags because each CPU belongs to one fixed region. Binary search
keeps established submissions allocation-free and lock-free for membership lookup, while table
storage scales with eligible processors rather than the highest ID. Processor-local pools
do not allocate this table. Expansion selects only already admitted CPUs plus the current
submitter from the exact destination region.

Queue insertion, membership and snapshot publication happen under the selected queue's
admission lock. Workers refresh after dequeueing but before invoking task code, so they
observe membership associated with that task. They also refresh between iterations and
recheck the revision after registering a sleep listener. Expansion wakes all idle workers
outside shared locks. Known submitters neither construct a mask nor broadcast a wakeup.

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
require heterogeneous task storage. Joined wrappers retain the original body and sender
together until execution consumes them. Dropping an unexecuted wrapper sends cancellation
with the original body, completing pending handles even while schedulers retain the registry.
Only cancellation boxes the body; normal execution uses the existing pooled task slot.
Recovering the owning heterogeneous slot would require moving ownership out of the queue's
handle rather than cancellation from the stored wrapper's destructor.

The public error owns its recovery state separately from its shared diagnostic source:
task captures and panic payloads need only be `Send`, not `Sync`. Error inspection exposes
no user state; extraction consumes the error. Task panic payloads retain their owning wrapper
with protected disposal, so dropping either channel endpoint or an unobserved panic error
cannot turn its payload destructor into an infrastructure failure. Extracting a panic transfers
disposal responsibility to the caller. Cancelled captures live in the result until recovery
or disposal, rather than being destroyed by queue draining while a handle remains alive.
