# cbh_codec implementation

`cbh_codec` implements the stored-object encoding required by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns deterministic gzip encoding and validation of encoded object bytes. It has no
storage backend or object-key policy; `cbh_storage` applies the codec at its persistence boundary,
and non-production data producers use the same encoding implementation.

Malformed encoded bytes produce `io::Error`. The storage adapter that knows the object and
operation adds semantic context under the workspace
[error-handling guide](../../../docs/error-handling.md).
