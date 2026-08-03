# cbh_storage - Implementation

`cbh_storage` implements persistence for the behavior described by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). One object-store port is
implemented by local filesystem, Azure Blob, read-through caching, and feature-gated in-memory
backends. Shared validation and key construction keep every backend in the same namespace.

Local writes compress data and publish it atomically. Each filesystem conversion boundary uses a
private operation-specific condition that carries the relevant path and retains the I/O or codec
error as its source. Missing reads and deletes preserve their operating-system causes while still
driving cache-miss behavior.

Azure operations add blob or configuration context and retain SDK diagnostics. The caching
backend uses per-project epoch markers to invalidate only the mirrored project after a remote
overwrite or deletion.

`StorageError` is the public transparent aggregate over private semantic conditions. Its private
total decision state distinguishes ordinary failures, missing objects, and already-existing
objects with their keys. The public API exposes only `is_not_found` and
`already_existing_key`, the two decisions used by production cache and write-once flows.
