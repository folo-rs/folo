# TODO

Tracking notes for follow-up work that is intentionally deferred. Each entry
should describe the task, the trigger condition that makes it actionable, and
links to the relevant code.

## Migrate to stable `std::hint::cold_path()` (requires MSRV 1.95+)

`packages/nm_impl/src/observations.rs` defines a private `cold_path()` helper as a
workaround for `std::hint::cold_path()` being unavailable at the workspace MSRV.

The intrinsic stabilized in Rust 1.95. The workspace toolchain
(`rust-toolchain.toml`) is at 1.96.1, but the workspace MSRV is 1.93.1, so
`std::hint::cold_path()` cannot yet be used without raising `nm`'s MSRV.

Once the workspace MSRV (or just `nm`'s package MSRV) is raised to 1.95 or
later:

1. Delete the private `fn cold_path()` helper from
   `packages/nm_impl/src/observations.rs`.
2. Replace each call site (`cold_path();`) with `std::hint::cold_path();`.
3. Verify with `just package=nm_impl bench-cg` that Callgrind numbers remain at
   or better than the stable-helper baseline. The stdlib version uses an
   LLVM intrinsic and should be at least as effective as the
   `#[cold] #[inline(never)] fn` workaround.

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
