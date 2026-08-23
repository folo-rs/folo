# cbh_probe implementation

`cbh_probe` implements the provenance and machine-partition support required by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns observation of repository, toolchain, and hardware facts and derivation of the
machine fingerprint from those facts. It delegates command execution to `cbh_git` and returns
shared facts through `cbh_model`, keeping observation separate from collection policy.

`SystemProbe` is best-effort: subprocess launch and exit failures become absent repository
observations or fallback toolchain observations rather than failing the probe. The
`EnvironmentProbe` trait retains `io::Result` so alternate implementations can report unexpected
failures for callers to contextualize under the workspace
[error-handling guide](../../../docs/error-handling.md).
