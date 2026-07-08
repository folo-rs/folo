#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The verbose-diagnostics channel and a few shared text helpers. A [`Reporter`]
//! surfaces the step-by-step detail behind a command when `--verbose` is set
//! (writing to standard error, or recording in memory for tests) without ever
//! contaminating machine-readable stdout, and [`count_noun`] spells out the
//! grammatically correct singular/plural form of a counted noun. Split out of the
//! `cargo-bench-history` shell so this deterministic, I/O-light code is cheap to
//! mutation-test in isolation.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_diag::StderrReporter` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod report;
mod text;

#[cfg(feature = "private-test-util")]
#[cfg_attr(docsrs, doc(cfg(feature = "private-test-util")))]
pub use report::RecordingReporter;
pub use report::{Notes, Reporter, ReporterExt, StderrReporter};
pub use text::count_noun;
