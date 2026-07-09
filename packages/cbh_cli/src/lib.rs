#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history shell crate, used only inside this workspace rather \
              than as a stable public API. Exhaustive construction and matching by \
              those in-workspace consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The argument-parsing surface: the [`clap`]-derived [`Cli`] that turns argv into the
//! typed [`Command`](cbh_command::Command) model, and the [`EarlyExit`] value that carries
//! a help listing or a parse error out to the process entry point. Split out of the
//! `cargo-bench-history` shell so the wide clap surface is isolated for mutation testing.
//!
//! Every item is re-exported flat from the crate root, so consumers write `cbh_cli::Cli`
//! rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod cli;

pub use cli::{Cli, EarlyExit};
