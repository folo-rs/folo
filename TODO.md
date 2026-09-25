# TODO

Tracking notes for follow-up work that is intentionally deferred. Each entry
should describe the task, the trigger condition that makes it actionable, and
links to the relevant code.

## Reorder bench-history object-key segments to `triple/machine/engine`

`cargo-bench-history` keys stored objects as
`v1/<project>/objects/<engine>/<target_triple>/<machine>/<commit>/…`
(`packages/cbh_model/src/comparability.rs`), putting the engine outermost.

Target triple and machine key identify completely independent data sets, so
they belong outermost, with the engines nested inside one machine's data. Under
the current order "everything this machine key recorded" is not a single
prefix, so a partition-scoped scan — such as `backfill`'s skip-existing
pre-check — needs one listing per engine instead of one listing overall.

Reordering rewrites every stored key, so it is not worth a storage-schema break
on its own. Do it as part of the next change that breaks the schema anyway.

## Enable newer Clippy module-root and bit-width lints

When the pinned Rust toolchain provides `clippy::definition_in_module_root` and
`clippy::manual_bit_width`, enable both as warnings in the root `Cargo.toml` and
adjust every reported violation. Rust 1.98.1 does not recognize either lint.

## Isolate remaining workflow concurrency groups by pull request

When updating general CI concurrency, replace source-branch grouping in
[Standard validation](.github/workflows/standard-validation.yml),
its [PR-close companion](.github/workflows/cancel-standard-validation.yml), and
[Book](.github/workflows/book.yml) with PR-number grouping. Same-named branches in
different forks can currently cancel each other's runs. Keep Standard validation
and its close companion's group expressions synchronized, including the companion's
reported group name, and preserve scheduled-run isolation.
