# events_once package guidelines

These guidelines document conventions, priorities and constraints specific to the
`events_once` package. They complement the workspace-wide guidance in the
repository root `AGENTS.md`.

## Performance priorities

When weighing optimization candidates, the relative importance of user-facing
scenarios is:

1. **Pooled events** (`EventPool::rent` / `LocalEventPool::rent`) — primary
   performance target. Anyone reaching for `events_once` for high throughput
   uses pooled events or events rented from an `EventLake`.
2. **Embedded events and events rented from an `EventLake`** — also primary.
   Embedded events live inside user storage with no allocator hop; events
   rented from a lake (`EventLake` / `LocalEventLake`) extend pooling to
   variable `T`.
3. **Boxed events** (`Event::boxed()` / `LocalEvent::boxed()`) — least
   important. Any performance-conscious user will use one of the variants
   above. Boxed events exist primarily for convenience and for cases where
   the caller cannot bound the event's lifetime to a specific scope.

Within all variants:

* **Steady-state send + receive on the happy path** dominates priority.
* **Cancellation paths** (sender dropped without setting, receiver dropped
  before polling) are secondary — they must not regress unboundedly, but
  optimizations targeting only cancellation are lower priority than wins on
  the happy path.

## LocalEvent is expected to beat thread-safe Event

The single-threaded `LocalEvent<T>` variant **must** be at least as fast as,
and ideally faster than, the thread-safe `Event<T>` variant on equivalent
operations.

`LocalEvent` does not perform atomic operations, does not contend on a mutex
when used with `LocalEventPool`, and does not need fences. If a benchmark
shows `LocalEvent` losing to `Event` on the same scenario, that is a bug or a
missed optimization opportunity, not a tolerable cost of the simpler
single-threaded design.

When adding new operations or refactoring existing ones, verify with
Callgrind that the local variant retains its performance edge.

## Benchmark coverage policy

The package's Callgrind benchmark suite is expected to cover every
combination of:

* event variant (sync `Event`, local `LocalEvent`)
* storage strategy (boxed, pooled, embedded / by-ref, rented from an
  `EventLake`)
* primary lifecycle outcome (send + receive happy path, sender dropped,
  receiver dropped before polling)

When undertaking optimization work in this package, fill any missing scenarios
proactively as part of the work. Do not gate optimization on the absence of
a benchmark — add the benchmark, then measure, then decide.

Per the repository convention, every Callgrind scenario should have an
analogous Criterion scenario where one is meaningful (the reverse is not
required); see the workspace-level `docs/callgrind-benchmarks.md`.

## `#[inline]` annotations have outsized impact in this package

Empirical evidence (see PR #194 and follow-ups): every layer of the event
machinery is a thin generic forwarder calling the next layer (e.g.
`Future::poll` -> `ReceiverCore::poll` -> `Event::poll` -> `EventRef::get` ->
`Deref::deref`). rustc's cross-crate inline heuristic refuses to export MIR
for functions that do not look like leaves, so the chain breaks at whichever
forwarder rustc decides to keep monomorphic.

When adding new methods on the public hot path (rent, send, poll, drop), or
when refactoring an existing one, run `just package=events_once bench-cg`
**before and after**. If an apparently trivial change moves more than ~5
instructions on the lifecycle benchmarks, suspect an inlining regression and
inspect the bench binary's disassembly with `objdump -d -Mintel -C` to
confirm whether the function is being inlined.

Inlining is asymmetric and not transitive: inlining a function with a body
that is too large into its caller can prevent the caller from being inlined
into ITS caller. When tempted to add `#[inline]` to a heavy method (e.g.
the `EventRef::release_event` impl for `BoxedRef` / `BoxedLocalRef`, which
deallocates), measure first — the "inline cascade" can regress overall
numbers even though the local symbol gets inlined.
