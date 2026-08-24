# `nm_impl`

Implementation crate for [`nm`](https://crates.io/crates/nm). Do not depend on this crate
directly.

This crate exists so the `nm` package can keep its published API surface minimal
while still permitting in-workspace tests and benchmarks to reach internal items
that should not appear on `docs.rs/nm`. The [`nm` design] and
[`nm` implementation guide] describe the behavior and architecture owned by the
package family. See the [`_impl` crate convention] for the broader workspace
pattern.

Only the items re-exported by `nm` participate in its public API contract. Other
items may be renamed, removed, or have their behavior changed at any time.
Downstream consumers should always depend on `nm` instead.

[`nm` implementation guide]:
    https://github.com/folo-rs/folo/blob/main/packages/nm/docs/implementation.md
[`nm` design]:
    https://github.com/folo-rs/folo/blob/main/packages/nm/docs/design.md
[`_impl` crate convention]:
    https://github.com/folo-rs/folo/blob/main/docs/impl-crate-split.md
