# nm implementation

The [`nm` design](design.md) defines the user-visible behavior owned by this package family.
This guide describes the internal boundaries that preserve that behavior.

## Package boundary

The published `nm` package is the public API shell. It selects and re-exports the public
surface implemented by the private `nm_impl` package. Keeping implementation dependencies
behind this shell prevents internal helpers and storage choices from becoming accidental
API while allowing examples, documentation, and re-export contract tests to live with the
owning package.

## Observation storage

Each event handle owns publication-model-specific observation storage and a cached
low-precision clock. Event handles remain single-threaded so observation can use local
mutation without synchronization overhead on the hot path.

Registration connects a thread-local event handle to process-wide reporting by event name
and histogram configuration. Per-thread uniqueness prevents ambiguous ownership of a local
registration. Cross-thread registrations with the same name are merged only when their
configurations are compatible. Thread teardown merges compatible observations into compact
archives and retains incompatible configurations separately, allowing report collection to
enforce the public configuration contract without panicking from a destructor.

## Publishing models

Pull publishing writes observations to storage that report collection can inspect directly.
It favors automatic visibility over the lowest possible observation overhead.

Push publishing writes to thread-local storage associated with a metrics pusher. Publication
copies the latest local state into report-visible storage. Unchanged event state can be
skipped during later publications because observation counts advance whenever data changes.
This favors a cheaper observation path while making publication an explicit owner
responsibility.

## Report assembly

Report collection snapshots registered report-visible storage, groups snapshots by event
name, validates compatible histogram configuration, and combines their count, sum, and
bucket counts. The resulting event metrics are sorted by name to make human-readable output
deterministic. Reports own their snapshots, so callers can inspect or render them without
holding registry access.

## API contract validation

The shell's integration test names every re-export and asserts the thread-safety and unwind
traits of concrete exported types and publication specializations. This guards item
reachability and type-level contracts at the public package boundary.
