# cbh_storage

Implementation crate for [`cargo-bench-history`](https://github.com/folo-rs/folo). Do
not depend on this directly — it carries the storage port and its backends (local
filesystem, Azure Blob, and a read-through cache that mirrors a cloud backend onto local
disk), plus the key-layout and cache-epoch rules that keep those backends consistent. It
has no stable public API; use the `cargo-bench-history` tool instead.
