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
        /// Workspace manifest to classify. Used verbatim.
        manifest_path: PathBuf,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
    /// `check` — fail on unreleased changes or an inconsistent group.
    Check {
        /// Base revision whose first-parent line supplies anchors.
        base: String,
        /// Workspace manifest to classify. Used verbatim.
        manifest_path: PathBuf,
        /// How to render diagnostics.
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
        /// Workspace manifest to edit. Used verbatim.
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
        /// Whether every publishable package and version group passed.
        passed: bool,
        /// Rendered diagnostics or a success summary.
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
/// CI should pass an explicit SHA of the merge-base or target-branch tip. A
/// local run compares against the default remote mainline. A stale default can
/// both add and hide differences, so it is not a conservative fallback.
pub(crate) const DEFAULT_BASE: &str = "origin/main";

/// Shared plan and report schema revision.
///
/// Plan and report formats advance together. Incompatible field, enum, or
/// path-layout changes increment this constant. Contract: package README
/// "Plan and report schema".
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Skill named in check failure text so a failing job is a sufficient prompt.
pub(crate) const INCREMENT_VERSIONS_SKILL: &str = "increment-versions";

/// Matches the user-facing short-commit convention in `cbh_detect`.
///
/// Ref: `packages/cbh_detect/src/detect/findings.rs`, `short_commit`.
pub(crate) const SHORT_COMMIT_LEN: usize = 12;

pub(crate) fn short_commit(commit: &str) -> &str {
    commit
        .get(..commit.len().min(SHORT_COMMIT_LEN))
        .unwrap_or(commit)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(CheckFormat: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RunInput: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RunOutcome: UnwindSafe, RefUnwindSafe);

    #[test]
    fn short_commit_truncates_long_revisions() {
        assert_eq!(short_commit("abcdefghijklmnop"), "abcdefghijkl");
        assert_eq!(short_commit("abc"), "abc");
    }
}
