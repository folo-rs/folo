# Agent notes for cargo-bench-history-faker

## What this crate is

A test-support stand-in benchmark engine for `cargo-bench-history`. It is a
`publish = true` **lib+bin** crate but **unsupported**: the crate root is
`#![doc(hidden)]` and neither the library API nor the binary CLI carries a semver
contract. It is published only so sibling repositories can run the binary (via
`cargo binstall`) without vendoring it.

## Layout

* `src/writers.rs` — the public library. Each engine has a **pure value core**
  (builds the on-disk document without touching the filesystem, so it is usable
  from unit tests and Miri) plus a thin `write_*` filesystem wrapper. The bin and
  in-workspace fidelity tests both call these, so there is one source of truth for
  the output shape.
* `src/main.rs` — a thin CLI over `writers`. It only parses arguments and calls
  library functions; keep logic out of it.
* `src/locate.rs` — the `binary_path()` locator used by `cargo-bench-history`'s
  tests to spawn the real binary. Gated behind `#[cfg(any(test, feature =
  "private-test-util"))]`, so it never ships as public API. In-workspace
  dev-dependencies enable `private-test-util`; the crate's own tests reach it via
  the `test` arm.

## Invariants

* **The Callgrind presets must match the committed parser fixtures.** The
  `single` / `parametrized` / `single-alt-pkg` presets in `writers.rs` reproduce
  the exact identity and `Ir`/`Bc`/`Bi` of
  `packages/cargo-bench-history/tests/fixtures/callgrind/*.summary.json`. If those
  fixtures change, update the presets in lockstep (and vice versa) — downstream
  `BenchmarkId` assertions depend on it.
* **The CLI flag surface is a contract with `cargo-bench-history`'s tests.** Many
  call sites pass `--summary`/`--criterion`/`--alloc-tracker`/`--all-the-time`/
  `--exit-code`/`--fail-if-exists`/`--chdir`. Renaming or removing a flag breaks
  them; grep the whole repo before changing one.
* **Version stays in the `cargo-bench-history` group** (see `release-plz.toml`), so
  a faker-only change bumps the whole CLI family. That is accepted.

## Building and testing

`cargo test -p cargo-bench-history-faker` builds and runs both the `writers` unit
tests and the `locate` tests (the `test` cfg activates the gated locator). No
feature flag is needed for the crate's own tests.
