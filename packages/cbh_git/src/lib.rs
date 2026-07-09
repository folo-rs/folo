#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an internal handoff boundary between the \
              cargo-bench-history sub-crates rather than a stable public API, so \
              exhaustive construction of its port value types by those in-workspace \
              consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The process and git-history ports. The process port launches the engine command
//! (streaming its output) and captures the output of helper commands such as `git` and
//! `rustc`; the git-history port gives `analyze` read-only access to a repository's
//! first-parent commit topology. Each has a real adapter backed by `tokio::process` and
//! an in-memory fake (behind `private-test-util`) so orchestration is testable without
//! spawning processes or a live repository. Split out of the `cargo-bench-history` shell
//! so this subprocess-adjacent code is isolated for mutation testing.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_git::SystemGitHistory` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod git;
mod git_history;
mod process;

pub use git::parse_git_info;
#[cfg(any(test, feature = "private-test-util"))]
#[cfg_attr(docsrs, doc(cfg(feature = "private-test-util")))]
pub use git_history::FakeGitHistory;
pub use git_history::{FirstParentCommit, GitHistory, SystemGitHistory};
pub use process::{BenchRunner, CommandOutput, EngineStatus, TokioBenchRunner, capture};
