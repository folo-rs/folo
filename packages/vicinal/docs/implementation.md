# vicinal implementation

User-visible behavior is defined in the package [design](design.md).

The pool maintains an indexed registry of worker groups created on demand. A typed
destination distinguishes logical processors from memory regions. Each submission samples
one destination and uses it for startup, allocation, queueing and diagnostics.

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

Cold startup and handle registration share a pool-level lock with the shutdown transition.
This lock is absent from steady-state submissions. Startup publishes its outcome only
after every named worker acknowledges successful affinity setup. Partial thread creation
or pinning failure marks the group terminal, signals already created workers and propagates
the panic outside the lock. All created handles remain accounted for until joined; failed
groups cannot accept work or silently retry.

Each group selects an affinity set once from `SystemHardware::all_processors()`, retaining
hard operating-system restrictions independently of processor-time quotas. A processor
group selects a singleton; a region group selects every eligible processor in that exact
region. Workers share the cached set and pin through the existing named-thread path.
Empty selections fail explicitly rather than executing unpinned.

Shutdown closes admission under the startup lock, signals groups, drains queues and joins
every registered handle. Enqueue checks the pool shutdown flag while holding its selected
queue lock. A group created after the shutdown scan cannot start workers or retain work.
Draining pops tasks under the queue lock and destroys them outside it; reentrant callbacks
observe the terminal pool state. Cleanup continues after a destructor or worker panic,
propagating the first failure only after resources are released and handles joined.
The terminal pool state takes precedence over a group's startup failure so every retained
scheduler follows the same abandonment path after shutdown.

Result channels use a separate typed pool because their layout and lifecycle do not
require heterogeneous task storage. Disconnecting abandoned senders completes pending
join handles even while scheduler clones retain the registry.
