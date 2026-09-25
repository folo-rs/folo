# future_deque implementation

User-visible behavior is defined in the package [design](design.md).

The two public collection variants share one deque core. Their behavioral logic is
identical; their insertion bounds and storage sources establish the difference in thread
mobility.

## Future storage

Each thread has separate heterogeneous pools for thread-mobile and local futures. Keeping
the pools separate prevents the variants from affecting each other's retained capacity.
Each concrete future layout is routed to reusable slots for that layout.

Insertion moves a future into stable pool storage and erases its concrete type behind an
owning handle. The handle owns the allocation independently of the thread-local pool
facade, so a deque and its futures may outlive the thread that inserted them. Completed or
removed entries release their individual slots for reuse.

The erased handle deliberately does not carry a thread-mobility marker so both variants
can use the same core. The thread-mobile variant restores the marker through its insertion
bounds and manual trait implementations.

## Polling and waking

Each pending entry owns a waker that records activation and forwards wakeups to the current
parent task. Activation state and parent-waker access remain thread-safe even for the local
collection because a standard waker may be cloned and invoked from another thread.

Waker metadata uses a separate typed pool because every entry has the same metadata layout.

Polling visits entries front-to-back and polls only entries whose activation state was
set. A completed future is replaced by its output value, releasing the pooled future
without changing its position in the deque.

## Benchmark boundaries

The local and thread-mobile collections use the same benchmark populations and future
shapes. Build-and-drain scenarios cover activity ratios, while transient churn holds
inactive entries resident to expose occupancy-sensitive polling and pool reuse.

Wake-burst scenarios capture every resident future's waker with an initial pending poll,
then enable the entire population and invoke all captured wakers before polling again.
Criterion and Callgrind measure that completion poll, including release of completed
future pool slots. A setup-owned readiness flag outlives the poll, so there is no
per-future heap deallocation. Construction, initial registration, waking and output
draining are excluded. The small and large populations expose per-ready-entry work without
introducing thread scheduling. Criterion uses bounded batches of populations to amortize
timer overhead; the measured deque is not necessarily the most recently prepared one.
