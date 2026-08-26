# vicinal implementation

User-visible behavior is defined in the package [design](design.md).

The pool maintains a registry of state created on demand for processors that submit work.
Each processor state owns its queues, wake and shutdown signals, task storage and result
channels. Workers pinned to that processor consume only its state.

## Task storage

Each processor owns a heterogeneous object pool that routes task layouts to reusable
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
therefore runs without any processor-state lock held.

Worker threads are created once per active processor and tracked by the owning pool.
Shutdown prevents new worker creation, wakes existing workers and joins every worker
thread. Result channels use a separate typed pool because their layout and lifecycle do
not require heterogeneous task storage.
