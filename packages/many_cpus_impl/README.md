# `many_cpus_impl`

Implementation crate for [`many_cpus`](https://crates.io/crates/many_cpus). Do not depend on
this crate directly.

This crate exists so the `many_cpus` package can keep its published API surface minimal
while still permitting in-workspace tests and benchmarks (in `many_cpus_impl` itself) to
reach internal items that should not appear on `docs.rs/many_cpus`. See
[`docs/impl-crate-split.md`](../../docs/impl-crate-split.md) in the repository for the
broader convention.

Anything beyond what `many_cpus` re-exports is **not** part of any public API contract:
items here may be renamed, removed, or change behavior at any time, including in patch
releases. Downstream consumers should always depend on `many_cpus` instead.
