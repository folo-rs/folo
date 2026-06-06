# `nm_impl`

Implementation crate for [`nm`](https://crates.io/crates/nm). Do not depend on this crate
directly.

This crate exists so the `nm` package can keep its published API surface minimal
while still permitting in-workspace tests and benchmarks (in `nm_impl` itself
and in `nm_otel`) to reach internal items that should not appear on
`docs.rs/nm`. See [`docs/impl-crate-split.md`](../../docs/impl-crate-split.md)
in the repository for the broader convention.

Anything beyond what `nm` re-exports is **not** part of any public API
contract: items here may be renamed, removed, or change behavior at any
time, including in patch releases. Downstream consumers should always depend
on `nm` instead.
