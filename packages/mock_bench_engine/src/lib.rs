//! Test-support crate providing a fake benchmark engine for the `cargo-bench-history`
//! integration tests.
//!
//! The fake engine itself is this crate's **binary** (`src/main.rs`); it writes
//! benchmark-engine output files into the target tree and exits with a caller-chosen
//! code, standing in for a real engine. This **library** exists only to let the
//! consuming tests locate that binary: Cargo exposes `CARGO_BIN_EXE_*` only to the
//! integration tests of the package that owns the binary, so `cargo-bench-history`'s
//! tests cannot reference it that way. Instead they take a normal `dev-dependency` on
//! this crate and call [`binary_path`], which builds the binary on demand and reports
//! its path.
//!
//! The whole crate is `publish = false`, so neither the binary nor this locator ever
//! ships, and `cargo install cargo-bench-history` places only the real tool on a
//! user's PATH (issue #289).
// The `coverage(off)` opt-out in `locate` lives only on the test module, so the feature is
// gated to test builds; declaring it unconditionally would be unused (and warn) when
// the library is compiled as a non-test dependency under coverage instrumentation.
#![cfg_attr(all(test, coverage_nightly), feature(coverage_attribute))]

mod locate;

pub use locate::binary_path;
