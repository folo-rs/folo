# `nm_otel_impl`

Implementation crate for [`nm_otel`](https://crates.io/crates/nm_otel). Do not depend on this
crate directly.

This crate exists so the `nm_otel` package can keep its published API surface minimal
while still permitting in-workspace tests and benchmarks (hosted here in `nm_otel_impl`)
to reach internal items that should not appear on `docs.rs/nm_otel`. See
[`docs/impl-crate-split.md`](../../docs/impl-crate-split.md) in the repository for the
broader convention.

Anything beyond what `nm_otel` re-exports is **not** part of any public API contract:
items here may be renamed, removed, or change behavior at any time, including in patch
releases. Downstream consumers should always depend on `nm_otel` instead.
