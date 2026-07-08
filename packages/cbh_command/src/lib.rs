#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history shell crate, used only inside this workspace rather \
              than as a stable public API. Exhaustive construction and matching by \
              those in-workspace consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The parsed command model: the [`Command`] enum and the per-subcommand option value
//! types (`CollectOptions`, `AnalyzeOptions`, and friends) plus the small selection enums
//! they embed. The CLI parser produces these values and the commands consume them, so
//! this is a dependency-light data layer between the two. Split out of the
//! `cargo-bench-history` shell so option-type mutation testing does not drag in the clap
//! parser or the analysis suite.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_command::Command` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod command;

pub use command::{
    AnalyzeOptions, BackfillOptions, BlessOptions, CacheSelection, CollectOptions, Command,
    ExamineOptions, InstallOptions, ListOptions, ListSubject, LocalStorageSelection, PruneOptions,
    UnblessOptions,
};
