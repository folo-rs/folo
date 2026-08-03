# cbh_model implementation

`cbh_model` provides the representation underlying the behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns the I/O-free vocabulary exchanged by collection, storage, and analysis: benchmark
identity, measurements, run context, persistence discriminants, stored records, and the persisted
object-key layout with its construction and parsing. Engine-specific schemas, backend-specific
representations, and storage-facing safety validation stay outside this boundary.

Domain validation and reduction operations expose aggregates where the model owns semantic
context. JSON conversion returns `serde_json::Error` directly because the model cannot identify
the caller's storage or command operation. That caller adds semantic context while preserving the
foreign source, in accordance with the workspace
[error-handling guide](../../../docs/error-handling.md).
