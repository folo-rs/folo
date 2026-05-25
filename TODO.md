# TODO

Tracking notes for follow-up work that is intentionally deferred. Each entry
should describe the task, the trigger condition that makes it actionable, and
links to the relevant code.

## Migrate to stable `std::hint::cold_path()` (Rust 1.95+)

`packages/nm/src/observations.rs` defines a private `cold_path()` helper as a
workaround for `std::hint::cold_path()` being unstable in the current
workspace toolchain (Rust 1.93, gated behind `feature(cold_path)`).

The intrinsic stabilized in Rust 1.95. Once the workspace toolchain
(`rust-toolchain.toml`) is upgraded to 1.95 or later:

1. Delete the private `fn cold_path()` helper from
   `packages/nm/src/observations.rs`.
2. Replace each call site (`cold_path();`) with `std::hint::cold_path();`.
3. Verify with `just package=nm bench-cg` that Callgrind numbers remain at
   or better than the stable-helper baseline. The stdlib version uses an
   LLVM intrinsic and should be at least as effective as the
   `#[cold] #[inline(never)] fn` workaround.
