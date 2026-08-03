# cbh_probe implementation

`cbh_probe` implements the provenance and machine-partition support required by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns observation of repository, toolchain, and hardware facts and derivation of the
machine fingerprint from those facts. It delegates command execution to `cbh_git` and returns
shared facts through `cbh_model`, keeping observation separate from collection policy.

Unexpected process failures remain `io::Error` values at this low-level boundary. Callers that
know the operation being attempted add semantic context under the workspace
[error-handling guide](../../../docs/error-handling.md).
