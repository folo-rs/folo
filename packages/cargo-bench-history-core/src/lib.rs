#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Pure internals of [`cargo-bench-history`]: the stored data model, the
//! comparability/partitioning rules, and the analysis math and rendering.
//!
//! This crate deliberately performs **no I/O** — no storage, git, process, or
//! filesystem access. Everything that touches the outside world (storage
//! backends, git history, benchmark-output harvesting, the CLI) lives in the
//! `cargo-bench-history` shell crate, which depends on this one and calls these
//! leaf functions with already-loaded inputs.
//!
//! The split exists so the large, I/O-free half of the tool can be exercised by a
//! fast in-process unit-test suite that is cheap to mutation-test, instead of
//! re-paying the shell's git-subprocess-heavy integration suite for every mutant.
//! The internals are themselves split across the `cbh_model`, `cbh_stats`,
//! `cbh_codec`, `cbh_analysis`, and `cbh_render` implementation crates; this crate
//! re-exports them under the three namespaces its consumers expect.
//!
//! The public surface is three namespaces: [`model`] (the data model and
//! comparability rules), [`analyze`] (the analysis math and rendering), and
//! [`codec`] (the gzip byte format the storage layer and the stress harness share
//! so they encode stored objects identically). Within each, every type is
//! re-exported flat, so consumers write `crate::model::Run` and
//! `crate::analyze::Finding` rather than reaching into private submodules.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

pub mod analyze;

#[cfg(feature = "private-test-util")]
#[cfg_attr(docsrs, doc(cfg(feature = "private-test-util")))]
pub use cbh_analysis::testing;
pub use cbh_codec as codec;
pub use cbh_model as model;
