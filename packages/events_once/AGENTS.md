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

That claim is about real execution speed, so the evidence for it is the
wall-clock measurement: every `events_once_ops` Criterion group registers the
local and thread-safe leaves side by side under one group name, and those are
the numbers that decide whether the requirement holds. Callgrind supplies the
explanation rather than the verdict — an instruction-count delta attributes the
difference to a code path, and the global bus event (`Ge`) reported by
`events_once_ops_cg` counts the lock-prefixed instructions that separate the
two variants. Never conclude from Callgrind alone that one variant is faster;
see `docs/callgrind-benchmarks.md`, "Cross-validate design decisions against
Criterion".

## Canonical benchmark scenario matrix

The package's benchmarks live in two paired files: `benches/events_once_ops.rs`
(Criterion, wall clock) and `benches/events_once_ops_cg.rs` (Callgrind,
instruction counts). A scenario is identified by the group it belongs to, the
threading model, the storage strategy, the start state, the timed operation and
the cleanup boundary. The Criterion identifier is
`events_once_ops/<group>/<model>[/<storage>]/<case>`; its Callgrind twin is the
same identifier with the file prefix dropped and `/` replaced by `_`.

Each group registers the **full cross product** of its cases, its storage rows
and its models, so the table below identifies every row that exists:

| Group | Cases | Storage rows | Models | Callgrind coverage |
| --- | --- | --- | --- | --- |
| `rent` | one: acquire endpoints from warmed storage | pooled | local, sync | every row (2) |
| `lifecycle` | one: acquire, send, poll out the value, release | boxed, embedded, pooled, raw_pooled, lake, raw_lake | local, sync, plus one `oneshot` leaf | every local and sync row (12) |
| `lifecycle_await_first` | one: acquire, poll (pending), send, poll out the value, release | as above | local, sync, plus one `oneshot` leaf | pooled only (2) |
| `send` | `bound`, `awaiting`, `disconnected` | boxed | local, sync | every row (6) |
| `poll` | `pending_first`, `pending_repeat`, `disconnected` | boxed | local, sync | every row (6) |
| `into_value` | `pending`, `ready`, `disconnected` | boxed | local, sync | every row (6) |
| `is_ready` | `pending`, `ready`, `disconnected` | boxed | local, sync | none |
| `cancel` | `sender_first_bound`, `sender_first_awaiting`, `receiver_first_bound` | boxed, embedded, pooled, raw_pooled, lake, raw_lake | local, sync | every row (36) |

That is 88 Criterion rows and 70 Callgrind rows. The `oneshot` leaves are an
external reference point for the same lifecycle; they live in the same group so
Criterion reports them alongside our own numbers.

What each group puts inside the measured region:

* `rent` measures one rental from a warmed managed pool. Endpoint destruction and
  the pool-handle drop happen after the measured region, so the row isolates
  reusable-slot acquisition, in-place initialization and endpoint construction.
* `lifecycle` and `lifecycle_await_first` measure acquisition through final
  release. For boxed events acquisition allocates and release frees; for the
  embedded rows acquisition is `Event::placed` into caller-owned storage that
  was prepared beforehand, so placement and release are measured but the
  storage allocation is not; the pooled, raw-pooled, lake and raw-lake rows
  rent from a warmed container and return the event to it.
* `send`, `poll`, `into_value` and `is_ready` measure one call against a peer
  that setup already brought to the named state. `send/disconnected`,
  `poll/disconnected`, `into_value/ready` and `into_value/disconnected` release
  the storage as part of that call, so their release is measured; every other
  row in these groups hands the surviving endpoint back untouched, `is_ready`
  in all three of its states because a readiness probe only reads state.
* `cancel` measures both endpoint drops, so the storage-specific final release
  performed by whichever endpoint goes last is inside the measurement.

Rules that keep the matrix coherent:

* Every Callgrind row has a Criterion twin prepared by an identically named
  setup function, with the same warm-up, the same storage and the same cleanup
  boundary. The reverse is not required; a Criterion row may stand alone.
* Nothing that is not the operation under test is built inside a measured
  region. Setup supplies warmed pools and lakes, endpoints in the named start
  state, caller-owned embedded storage, and the noop-waker polling context,
  which carries no per-event state and is therefore prepared once per row.
* Whatever owns the storage — a pool handle, a lake handle, or the caller's
  embedded place — travels with the endpoints and is handed back out of the
  measured region, so container teardown is never timed.
* Setup functions hand over unboxed endpoints; a scenario that needs a pinned
  receiver pins it with `Pin::new`. A benchmark-owned `Box` freed inside a
  measured region would show up as storage-release cost that no user pays.
* The storage sweep lives where release is timed. `cancel` sweeps all six
  strategies because release is exactly what differs between them; the focused
  groups use boxed storage alone because their bound and awaiting cases never
  reach storage, and their terminal cases would only repeat what `cancel`
  already separates.
* `lifecycle_await_first` gets its Callgrind coverage on pooled storage only:
  what distinguishes it from the send-first lifecycle lives in the event state
  machine, which every storage strategy shares, and pooled is the package's
  primary performance target. Its Criterion group keeps the full storage sweep
  because it also hosts the third-party comparison.
* `is_ready` is measured on real hardware only: it is a single state read, so
  an instruction count at that magnitude reports the harness, not the operation.
* Correctness assertions never appear inside a measured region. Results are
  consumed through `black_box`; behavior is verified by the test suite.

When undertaking optimization work in this package, fill any scenario the
matrix promises but does not yet register, and extend the matrix itself when a
new operation or storage strategy appears. Do not gate optimization on the
absence of a benchmark — add the row, then measure, then decide.

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
confirm whether the function is being inlined. Callgrind and disassembly
attribute the delta to generated code; whether the change is faster for a user
is decided by running the paired Criterion group (`just package=events_once
bench`) on the same scenario.

Inlining is asymmetric and not transitive: inlining a function with a body
that is too large into its caller can prevent the caller from being inlined
into ITS caller. When tempted to add `#[inline]` to a heavy method (e.g.
the `EventRef::release_event` impl for `BoxedRef` / `BoxedLocalRef`, which
deallocates), measure first — the "inline cascade" can regress overall
numbers even though the local symbol gets inlined.
