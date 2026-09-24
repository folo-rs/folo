# cbh_analyze implementation

`cbh_analyze` implements the query and mutation behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

This crate owns query and mutation orchestration: selecting and loading stored data, resolving
repository history, coordinating blessings and pruning, and assembling requested outputs. Shared
dataset-selection capabilities keep the query commands aligned where the application contract
requires common behavior. It delegates I/O-free series construction and detection to `cbh_detect`
and report presentation to `cbh_render`.

Analysis carries the renderer-owned outcome with the rendered report bundle so the shell can
expose it in process or write the requested outcome file. It uses the
[shared projection](../../cbh_render/docs/implementation.md), not an orchestration-specific
mapping from findings and series coverage.

The public command entry points own production wiring: they resolve and construct the configured
storage, repository, diagnostics, environment, time, and task-execution capabilities before
delegating. Their inner `*_with` orchestrators receive generic ports and explicit runtime values,
which keeps policy deterministic and same-crate tests in memory. The component crates own the
adapter implementations; `cbh_analyze` selects and coordinates them.

The query entry points acquire the quota-respecting processor count from `many_cpus` alongside
the production spawner, including processors across Windows processor groups. The process-wide
default set respects hard resource limits rather than inheriting the calling thread's soft
affinity: tasks may execute on other runtime threads. This query does not change thread affinity
or runtime sizing. The processor set is nonempty. That nonzero capacity is passed through
dataset loading and detection, each capping its worker count at its input length. Inner
orchestrators and their in-memory tests supply execution capacity explicitly rather than
discovering hardware during partitioning. Seeded-history integration tests exercise the real
command wiring and Tokio execution without running a benchmark engine.

Every command selects the ordinary storage facade. Read-only queries synchronize its optional
cloud cache before the shared selection pipeline and report cache activity afterward.
Git-topology selection separates the target branch's measurements from the base-ref history
within that store; unrelated branch commits do not enter the comparison.

Operations cross the crate boundary through a transparent aggregate. Concrete conditions remain
private to the responsibility that owns their context, while component failures remain attached
as sources. The shell can therefore convert the aggregate into `ohno::AppError` without exposing
an internal taxonomy or losing causal diagnostics. The boundary follows the workspace
[error-handling guide](../../../docs/error-handling.md).
