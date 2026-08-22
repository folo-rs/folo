# vicinal design

`vicinal` executes synchronous tasks near the processor that submitted them. This keeps
detached work close to the data and caches used by its caller.

## Processor locality

Each task is assigned to the current processor when it is spawned. Workers associated with
that processor execute the task. Platforms without processor-pinning support retain worker
pool behavior without the locality guarantee.

Workers are created lazily because single-task latency on an otherwise idle pool is the
primary optimization target.

## Scheduling

Regular and urgent work use separate priority classes. Workers prefer urgent work, but
tasks within a class may execute in any order and urgent work does not preempt a task that
is already running.

Tasks with join handles return their output asynchronously. A task panic is captured and
resumed when the handle is awaited. Fire-and-forget tasks discard their output and report
panics through tracing.

## Shutdown

Dropping the pool signals its workers and waits for tasks that are already executing.
Queued tasks may be abandoned. Callers that require completion await the corresponding
join handles before dropping the pool.
