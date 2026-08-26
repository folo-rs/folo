// Public API types for cargo-release-plan.
//
// These types are used by main.rs and exposed via the crate's public API so that
// integration tests can exercise the core logic without spawning a subprocess.

use std::path::PathBuf;

/// Output format for `cargo release-plan check`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Hidden enum for internal/test use only"
)]
pub enum CheckFormat {
    /// Human-readable lines on stderr when the check fails.
    Text,
    /// GitHub Actions workflow annotations in addition to the text lines.
    Github,
}

/// Input parameters for [`run`](crate::run).
#[doc(hidden)]
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Hidden enum for internal/test use only"
)]
pub enum RunInput {
    /// `report` — write `report.json` and per-package diffs.
    Report {
        /// Directory that receives `report.json` and `diffs/`.
        out_dir: PathBuf,
        /// Base revision whose first-parent line supplies anchors.
        base: String,
        /// Workspace manifest to classify. Defaults to `Cargo.toml` in the current directory.
        manifest_path: PathBuf,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
    /// `check` — fail on unreleased changes or an inconsistent group.
    Check {
        /// Base revision whose first-parent line supplies anchors.
        base: String,
        /// Workspace manifest to classify. Defaults to `Cargo.toml` in the current directory.
        manifest_path: PathBuf,
        /// How to render offences.
        format: CheckFormat,
        /// When set, warn on divergence from `cargo package --list` without failing.
        verify_packaging: bool,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
    /// `apply` — rewrite manifests according to an approved plan.
    Apply {
        /// Path to the plan JSON file.
        plan: PathBuf,
        /// When set, compute edits without writing files or refreshing the lockfile.
        dry_run: bool,
        /// Workspace manifest to edit. Defaults to `Cargo.toml` in the current directory.
        manifest_path: PathBuf,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
}

/// The successful outcome of a run.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Hidden enum for internal/test use only"
)]
pub enum RunOutcome {
    /// `report` finished and wrote its artifacts.
    Report {
        /// Human-readable summary for stdout. Empty when there is nothing to say.
        message: String,
    },
    /// `check` finished. `passed` is the process-level verdict.
    Check {
        /// `true` when every publishable package is `releasing` or `released` and
        /// every version group is consistent.
        passed: bool,
        /// Rendered offences or a success summary.
        message: String,
    },
    /// `apply` finished (including `--dry-run`).
    Apply {
        /// Human-readable summary for stdout.
        message: String,
    },
}

/// Default base revision when the caller does not pass `--base`.
///
/// CI passes an explicit SHA. A local run compares against the default remote
/// mainline. A stale base can only move anchors further back, which reports
/// more, never less.
pub(crate) const DEFAULT_BASE: &str = "origin/main";

/// Plan / report schema version this tool reads and writes.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Skill named in check failure text so a failing job is a sufficient prompt.
pub(crate) const INCREMENT_VERSIONS_SKILL: &str = "increment-versions";

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(CheckFormat: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RunInput: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RunOutcome: UnwindSafe, RefUnwindSafe);
}
