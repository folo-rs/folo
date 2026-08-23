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

The public command entry points own production wiring: they resolve and construct the configured
storage, repository, diagnostics, environment, time, and task-execution capabilities before
delegating. Their inner `*_with` orchestrators receive generic ports and explicit runtime values,
which keeps policy deterministic and same-crate tests in memory. The component crates own the
adapter implementations; `cbh_analyze` selects and coordinates them.

Operations cross the crate boundary through a transparent aggregate. Concrete conditions remain
private to the responsibility that owns their context, while component failures remain attached
as sources. The shell can therefore convert the aggregate into `ohno::AppError` without exposing
an internal taxonomy or losing causal diagnostics. The boundary follows the workspace
[error-handling guide](../../../docs/error-handling.md).
