# TODO

Tracking notes for follow-up work that is intentionally deferred. Each entry
should describe the task, the trigger condition that makes it actionable, and
links to the relevant code.

## Migrate to stable `std::hint::cold_path()` (requires MSRV 1.95+)

`packages/nm/src/observations.rs` defines a private `cold_path()` helper as a
workaround for `std::hint::cold_path()` being unavailable at the workspace MSRV.

The intrinsic stabilized in Rust 1.95. The workspace toolchain
(`rust-toolchain.toml`) is at 1.96.1, but the workspace MSRV is 1.93, so
`std::hint::cold_path()` cannot yet be used without raising `nm`'s MSRV.

Once the workspace MSRV (or just `nm`'s package MSRV) is raised to 1.95 or
later:

1. Delete the private `fn cold_path()` helper from
   `packages/nm/src/observations.rs`.
2. Replace each call site (`cold_path();`) with `std::hint::cold_path();`.
3. Verify with `just package=nm bench-cg` that Callgrind numbers remain at
   or better than the stable-helper baseline. The stdlib version uses an
   LLVM intrinsic and should be at least as effective as the
   `#[cold] #[inline(never)] fn` workaround.
