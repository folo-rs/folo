//! `cargo-bench-history-faker`: a synthetic benchmark-output generator.
//!
//! This crate writes criterion / Callgrind-summary / `alloc_tracker` /
//! `all_the_time` output files into a cargo target tree, imitating the parts of a
//! real benchmark engine that `cargo-bench-history` observes. It exists to validate
//! the `cargo-bench-history` storage and analysis pipeline end to end against
//! curated engine output (for example, via `cargo bench-history import`), in this
//! repository and in any other repository of the organization.
//!
//! It is **unsupported and carries no stability guarantee**: the library API and
//! the binary's command line may change at any time, in any release, without a
//! semver major bump. Do not depend on either as a stable contract. The crate is
//! published only so sibling repositories can run the binary (and, via
//! `cargo binstall`, fetch a prebuilt one) without vendoring it.
//!
//! The whole crate root is `#[doc(hidden)]` to keep this unsupported surface out of
//! the rendered documentation.
#![doc(hidden)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// This crate is test-support scaffolding — a synthetic benchmark-output generator
// driven by in-workspace tests and by the published binary. Its code carries no
// product behavior worth a coverage signal, so coverage is disabled crate-wide
// rather than per function.
#![cfg_attr(coverage_nightly, coverage(off))]

mod writers;

pub use writers::{
    AllocOperation, CallgrindCase, CriterionCase, TimeOperation, callgrind_summary,
    parse_alloc_arg, parse_callgrind_arg, parse_criterion_arg, parse_time_arg, write_all_the_time,
    write_alloc_tracker, write_callgrind_case, write_callgrind_cases, write_callgrind_summary,
    write_criterion_case, write_criterion_cases,
};

// `binary_path` shells out to `cargo build` to locate the compiled binary. Only
// in-workspace tests that must spawn the *real* binary need it, so it is gated
// behind `private-test-util` (never a public API; see docs/impl-crate-split.md).
// The `any(test, ...)` arm keeps this crate's own locator tests building without
// the feature.
#[cfg(any(test, feature = "private-test-util"))]
mod locate;
#[cfg(any(test, feature = "private-test-util"))]
pub use locate::binary_path;
