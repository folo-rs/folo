# vicinal implementation

User-visible behavior is defined in the package [design](design.md).

The pool maintains an indexed registry of worker groups created on demand. A typed
destination distinguishes logical processors from memory regions. Each submission samples
one processor and derives its regional destination from the same cached topology entry.
This coherent target is reused for startup, allocation, queueing and diagnostics.

Each group owns its queues, wake and shutdown signals, task storage, result channels and
startup outcome. Workers consume only their group's queues. The registry uses the selected
policy's hardware index-space bound, not a count of populated regions; sparse IDs and
regions without eligible processors do not imply worker creation. Initialized lookups
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
revision when admitting work from a new CPU. A pool-wide indexed atomic membership table
is sufficient because each CPU belongs to one fixed region. Expansion selects only already
admitted CPUs plus the current submitter from the exact destination region.

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

The submission boundary covers topology sampling, result channels, task allocation and
admission. Infrastructure panics unwind only after local guards have been released, then
stop the pool and propagate. Worker entry has the equivalent boundary around infrastructure
execution. Task wrappers separately catch user-task panics, keeping them out of the
infrastructure boundary. Cleanup failures also poison the pool while cleanup continues.
Workers check the pool-wide stop flag before task execution, rather than waiting for the
group signal scan to reach their group.

Shutdown closes admission under the startup lock, signals groups, drains queues and joins
every registered handle. Enqueue checks the pool shutdown flag while holding its selected
queue lock. A group created after the shutdown scan cannot start workers or retain work.
Draining pops tasks under the queue lock and destroys them outside it; reentrant callbacks
observe the terminal pool state. Cleanup continues after a destructor or worker panic,
propagating the first failure only after resources are released and handles joined.
Clean shutdown permits retained schedulers to abandon submissions normally. Poisoning
takes precedence over shutdown, permanently rejecting every submission with a panic.

Result channels use a separate typed pool because their layout and lifecycle do not
require heterogeneous task storage. Disconnecting abandoned senders completes pending
join handles even while scheduler clones retain the registry.
