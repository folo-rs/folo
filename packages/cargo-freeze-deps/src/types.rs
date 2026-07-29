// Public API types for cargo-freeze-deps.
//
// These types are used by main.rs and exposed via the crate's public API so that integration
// tests can exercise the core logic without spawning a subprocess.

use std::path::PathBuf;

/// Input parameters for the [`run`](crate::run) function.
#[doc(hidden)]
#[derive(Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "Hidden struct for internal/test use only"
)]
pub struct RunInput {
    /// Path to the Cargo.toml file to read.
    pub path: PathBuf,
    /// Optional path to write the rewritten Cargo.toml to.
    ///
    /// When `None`, the file at `path` is rewritten in place.
    pub output: Option<PathBuf>,
}

/// The successful outcome of a run.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "Hidden struct for internal/test use only"
)]
pub struct RunOutcome {
    /// Number of dependency version requirements that were rewritten.
    pub frozen_count: usize,
    /// Number of dependency version requirements that were left as-is because they
    /// did not have any freezable component (e.g. `<1.2.3`, bare `*`).
    pub skipped_count: usize,
}
