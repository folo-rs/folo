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

Logical blessings pair the common acceptance payload with a required scope. Empty
axis arrays mean unrestricted; populated arrays use the same case-insensitive
matching rule as queries. Intersection supports inspection and application, while
containment protects revocation from affecting unselected partitions.

Their keys occupy `objects/blessings/<commit>/bless-<issued_unix_nanos>.json`, independent
of measurement partitions but inside the data subtree covered by cache invalidation.
The nanosecond issue time distinguishes invocations; storage must use write-once
insertion so even a colliding timestamp cannot overwrite acceptance. Partition-local
records keep their existing keys and payloads; their scope comes only from the key.
The model parses both key forms separately so a logical scope cannot become a
measurement partition or a discovered machine.

`CollectionSnapshot` is the independently versioned handoff for one clean execution.
It contains full engine payloads plus project, commit, target and hardware identity, including
identity for a successful empty collection. Its strict decoder validates identity consistency,
unique engines, benchmark IDs and metric kinds, and finite measurements. Unlike historical
`Run` decoding, unknown metric kinds are errors: silently dropping them would change the
claimed current roster. Snapshot values come from collection finalization, never a store read.

Domain validation and reduction operations expose aggregates where the model owns semantic
context. JSON conversion returns `serde_json::Error` directly because the model cannot identify
the caller's storage or command operation. That caller adds semantic context while preserving the
foreign source, in accordance with the workspace
[error-handling guide](../../../docs/error-handling.md).
