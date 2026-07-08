#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form a handoff boundary to the \
              cargo-bench-history shell crate (and that crate's tests), used only \
              inside this workspace rather than as a stable public API. Exhaustive \
              construction and matching by those in-workspace consumers is intended"
)]

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
pub mod model;

pub use cbh_codec as codec;

#[cfg(feature = "private-test-util")]
#[cfg_attr(docsrs, doc(cfg(feature = "private-test-util")))]
pub mod testing;
