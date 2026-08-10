# cbh_detect implementation

`cbh_detect` implements the analysis behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns the I/O-free transformation from loaded run data and repository facts into series
and findings. It composes the kernels in `cbh_stats` with analysis-specific grouping, gating, and
ranking policy. Storage loading and history queries remain in `cbh_analyze`, while presentation of
the resulting findings remains in `cbh_render`.

Parallel work is supplied through an executor abstraction, preserving the same deterministic
analysis logic for production execution and synchronous component tests.
