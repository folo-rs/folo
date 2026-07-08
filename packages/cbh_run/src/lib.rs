#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history shell crate, used only inside this workspace rather \
              than as a stable public API. Exhaustive construction and matching by \
              those in-workspace consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The command run edge: the [`RunOutcome`] a successful run returns and the [`RunError`]
//! it fails with, plus the [`OutputWriter`] port that writes the per-format reports
//! (`--markdown`/`--json`/`--markdown-summary`) that the reporting commands emit. Sits
//! below the analyze orchestration and above config/storage, aggregating their errors
//! into `RunError`. Split out of the `cargo-bench-history` shell so this run/output
//! layer is cheap to mutation-test in isolation.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_run::RunError` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod outcome;
mod output;
mod paths;

pub use outcome::{RunError, RunOutcome, finish_with_flush};
#[cfg(any(test, feature = "private-test-util"))]
pub use output::MemoryOutputWriter;
pub use output::{OutputSelection, OutputWriter, TokioOutputWriter, emit, emit_markdown_summary};
pub use paths::rebase;
