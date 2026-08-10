# cbh_stats implementation

`cbh_stats` supports the analysis behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns pure statistical kernels used by `cbh_detect`. It implements mathematical
operations without repository, benchmark, storage, or reporting policy. This boundary keeps the
numerical layer deterministic and independently testable while leaving detector composition and
interpretation to the analysis engine.
