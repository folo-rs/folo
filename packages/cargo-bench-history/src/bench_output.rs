//! The benchmark-output port: collecting the machine-readable summary files an
//! engine wrote during a run, filtered to those produced by this run.
//!
//! The real adapter walks the cargo target tree with `tokio::fs`; an in-memory
//! fake (in `#[cfg(test)]`) returns canned summaries so orchestration is testable.

use std::ffi::OsStr;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::bench::{
    CRITERION_BENCHMARK_FILE, CRITERION_DIR, CRITERION_ESTIMATES_FILE, CRITERION_NEW_DIR,
    GUNGRAUN_DIR, SUMMARY_FILE,
};
use crate::comparability::EngineSystem;

/// Tolerance subtracted from the run-start boundary before comparing file
/// modification times, absorbing coarse filesystem mtime granularity so a summary
/// written moments after the run started is never mistaken for a stale one.
const MTIME_SLACK: Duration = Duration::from_secs(2);

/// One harvested Callgrind summary file: its path and raw contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawSummary {
    /// Filesystem path the summary was read from.
    pub(crate) path: PathBuf,
    /// Raw file contents (engine-specific JSON).
    pub(crate) content: String,
}

/// One harvested Criterion result case: the `new/` directory and the raw contents
/// of the `benchmark.json` and `estimates.json` files it pairs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawCriterionCase {
    /// The `new/` directory the case was read from.
    pub(crate) dir: PathBuf,
    /// Raw contents of `benchmark.json` (the case identity).
    pub(crate) benchmark: String,
    /// Raw contents of `estimates.json` (the statistical estimates).
    pub(crate) estimates: String,
}

/// The output harvested for a run, in the shape each engine produces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Harvest {
    /// Callgrind (Gungraun) summary files.
    Callgrind(Vec<RawSummary>),
    /// Criterion benchmark/estimates pairs.
    Criterion(Vec<RawCriterionCase>),
}

/// Collects the output an engine produced during a run.
pub(crate) trait BenchOutputSource {
    /// Returns every output `engine` wrote at or after `since`.
    ///
    /// # Errors
    ///
    /// Returns an error if the output tree cannot be read.
    fn collect(
        &self,
        engine: EngineSystem,
        since: SystemTime,
    ) -> impl Future<Output = io::Result<Harvest>>;
}

/// The real [`BenchOutputSource`], walking the cargo target tree.
#[derive(Clone, Debug)]
pub(crate) struct FsBenchOutputSource {
    target_root: PathBuf,
}

impl FsBenchOutputSource {
    /// Creates a source rooted at the cargo target directory `target_root`.
    pub(crate) fn new(target_root: impl Into<PathBuf>) -> Self {
        Self {
            target_root: target_root.into(),
        }
    }

    /// Walks `{target_root}/gungraun` for fresh `summary.json` files.
    async fn collect_callgrind(&self, since: SystemTime) -> io::Result<Vec<RawSummary>> {
        let threshold = since.checked_sub(MTIME_SLACK).unwrap_or(since);
        let summary_name = OsStr::new(SUMMARY_FILE);

        let mut summaries = Vec::new();
        let mut stack = vec![self.target_root.join(GUNGRAUN_DIR)];

        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };

            while let Some(entry) = entries.next_entry().await? {
                let file_type = entry.file_type().await?;
                let path = entry.path();
                if file_type.is_dir() {
                    stack.push(path);
                } else if path.file_name() == Some(summary_name) {
                    let modified = entry.metadata().await?.modified()?;
                    if modified >= threshold {
                        let content = tokio::fs::read_to_string(&path).await?;
                        summaries.push(RawSummary { path, content });
                    }
                }
            }
        }

        summaries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(summaries)
    }

    /// Walks `{target_root}/criterion` for fresh `new/` result directories.
    ///
    /// Criterion stores each benchmark case under `.../<case>/new/` (the most
    /// recent run) alongside a `base/` directory (the previous run); only `new/`
    /// directories holding both `benchmark.json` and `estimates.json` are
    /// harvested, and a case is admitted only when its `estimates.json` is no
    /// older than the run-start boundary. Incomplete pairs are skipped.
    async fn collect_criterion(&self, since: SystemTime) -> io::Result<Vec<RawCriterionCase>> {
        let threshold = since.checked_sub(MTIME_SLACK).unwrap_or(since);
        let benchmark_name = OsStr::new(CRITERION_BENCHMARK_FILE);
        let estimates_name = OsStr::new(CRITERION_ESTIMATES_FILE);
        let new_dir_name = OsStr::new(CRITERION_NEW_DIR);

        let mut cases = Vec::new();
        let mut stack = vec![self.target_root.join(CRITERION_DIR)];

        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };

            let mut has_benchmark = false;
            let mut estimates_mtime: Option<SystemTime> = None;
            while let Some(entry) = entries.next_entry().await? {
                let file_type = entry.file_type().await?;
                let path = entry.path();
                if file_type.is_dir() {
                    stack.push(path);
                } else if path.file_name() == Some(benchmark_name) {
                    has_benchmark = true;
                } else if path.file_name() == Some(estimates_name) {
                    estimates_mtime = Some(entry.metadata().await?.modified()?);
                }
            }

            // A complete, fresh case lives in a `new/` directory holding both files.
            if dir.file_name() != Some(new_dir_name) || !has_benchmark {
                continue;
            }
            let Some(modified) = estimates_mtime else {
                continue;
            };
            if modified >= threshold {
                let benchmark =
                    tokio::fs::read_to_string(dir.join(CRITERION_BENCHMARK_FILE)).await?;
                let estimates =
                    tokio::fs::read_to_string(dir.join(CRITERION_ESTIMATES_FILE)).await?;
                cases.push(RawCriterionCase {
                    dir,
                    benchmark,
                    estimates,
                });
            }
        }

        cases.sort_by(|left, right| left.dir.cmp(&right.dir));
        Ok(cases)
    }
}

impl BenchOutputSource for FsBenchOutputSource {
    async fn collect(&self, engine: EngineSystem, since: SystemTime) -> io::Result<Harvest> {
        match engine {
            EngineSystem::Callgrind => Ok(Harvest::Callgrind(self.collect_callgrind(since).await?)),
            EngineSystem::Criterion => Ok(Harvest::Criterion(self.collect_criterion(since).await?)),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;

    fn write_summary(root: &std::path::Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(GUNGRAUN_DIR).join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn write_criterion_file(root: &std::path::Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(CRITERION_DIR).join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn set_mtime(path: &std::path::Path, when: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    fn callgrind_summaries(harvest: Harvest) -> Vec<RawSummary> {
        match harvest {
            Harvest::Callgrind(summaries) => summaries,
            Harvest::Criterion(_) => panic!("expected callgrind harvest"),
        }
    }

    fn criterion_cases(harvest: Harvest) -> Vec<RawCriterionCase> {
        match harvest {
            Harvest::Criterion(cases) => cases,
            Harvest::Callgrind(_) => panic!("expected criterion harvest"),
        }
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn collects_fresh_summaries_recursively() {
        let dir = tempdir().unwrap();
        write_summary(dir.path(), "group_a/summary.json", "a");
        write_summary(dir.path(), "group_b/nested/summary.json", "b");
        write_summary(dir.path(), "group_a/other.json", "ignored");

        let source = FsBenchOutputSource::new(dir.path());
        let since = SystemTime::now() - Duration::from_mins(1);
        let harvest = source
            .collect(EngineSystem::Callgrind, since)
            .await
            .unwrap();

        let summaries = callgrind_summaries(harvest);
        let contents: Vec<&str> = summaries.iter().map(|s| s.content.as_str()).collect();
        assert_eq!(contents, vec!["a", "b"]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn excludes_summaries_older_than_the_boundary() {
        let dir = tempdir().unwrap();
        let stale = write_summary(dir.path(), "group/summary.json", "stale");

        // Backdate the file well beyond the mtime slack so it is unambiguously stale.
        let long_ago = SystemTime::now() - Duration::from_hours(1);
        set_mtime(&stale, long_ago);

        let source = FsBenchOutputSource::new(dir.path());
        let harvest = source
            .collect(EngineSystem::Callgrind, SystemTime::now())
            .await
            .unwrap();

        assert!(
            callgrind_summaries(harvest).is_empty(),
            "stale summary should be excluded"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn slack_window_includes_just_inside_and_excludes_just_outside() {
        let dir = tempdir().unwrap();
        let inside = write_summary(dir.path(), "fresh/summary.json", "inside");
        let outside = write_summary(dir.path(), "stale/summary.json", "outside");

        // The harvest admits files whose mtime is no older than `since - 2s`; the
        // slack absorbs coarse filesystem mtime resolution. Stamp one summary 1s
        // before `since` (inside the band → kept) and one 3s before (past it →
        // dropped), pinning both the slack magnitude and the boundary's inclusivity.
        let since = SystemTime::now();
        set_mtime(&inside, since - Duration::from_secs(1));
        set_mtime(&outside, since - Duration::from_secs(3));

        let source = FsBenchOutputSource::new(dir.path());
        let harvest = source
            .collect(EngineSystem::Callgrind, since)
            .await
            .unwrap();

        let summaries = callgrind_summaries(harvest);
        let contents: Vec<&str> = summaries.iter().map(|s| s.content.as_str()).collect();
        assert_eq!(contents, vec!["inside"]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn missing_output_tree_yields_no_summaries() {
        let dir = tempdir().unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let harvest = source
            .collect(EngineSystem::Callgrind, SystemTime::now())
            .await
            .unwrap();

        assert!(callgrind_summaries(harvest).is_empty());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn unreadable_output_tree_reports_error() {
        let dir = tempdir().unwrap();
        // Place a FILE where the gungraun output directory is expected so the
        // recursive read_dir fails with a non-NotFound I/O error.
        std::fs::write(dir.path().join(GUNGRAUN_DIR), "not a directory").unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let error = source
            .collect(EngineSystem::Callgrind, SystemTime::UNIX_EPOCH)
            .await
            .unwrap_err();

        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn criterion_collects_fresh_new_dirs_only() {
        let dir = tempdir().unwrap();
        // Two complete cases under `new/`, plus a stale `base/` sibling that must
        // be ignored (it also holds benchmark/estimates but is not a `new/` dir).
        write_criterion_file(dir.path(), "grp/std/now/new/benchmark.json", "bm-std");
        write_criterion_file(dir.path(), "grp/std/now/new/estimates.json", "est-std");
        write_criterion_file(dir.path(), "grp/std/now/base/benchmark.json", "bm-base");
        write_criterion_file(dir.path(), "grp/std/now/base/estimates.json", "est-base");
        write_criterion_file(dir.path(), "grp/fast/now/new/benchmark.json", "bm-fast");
        write_criterion_file(dir.path(), "grp/fast/now/new/estimates.json", "est-fast");

        let source = FsBenchOutputSource::new(dir.path());
        let since = SystemTime::now() - Duration::from_mins(1);
        let harvest = source
            .collect(EngineSystem::Criterion, since)
            .await
            .unwrap();

        let cases = criterion_cases(harvest);
        let pairs: Vec<(&str, &str)> = cases
            .iter()
            .map(|case| (case.benchmark.as_str(), case.estimates.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![("bm-fast", "est-fast"), ("bm-std", "est-std")],
            "only fresh new/ directories should be harvested, sorted by path"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn criterion_skips_incomplete_and_stale_cases() {
        let dir = tempdir().unwrap();
        // A case missing estimates.json (incomplete) and a fresh complete one.
        write_criterion_file(dir.path(), "grp/incomplete/now/new/benchmark.json", "bm-x");
        write_criterion_file(dir.path(), "grp/fresh/now/new/benchmark.json", "bm-fresh");
        let estimates =
            write_criterion_file(dir.path(), "grp/fresh/now/new/estimates.json", "est-fresh");
        // A complete but stale case whose estimates predate the boundary.
        write_criterion_file(dir.path(), "grp/stale/now/new/benchmark.json", "bm-stale");
        let stale_estimates =
            write_criterion_file(dir.path(), "grp/stale/now/new/estimates.json", "est-stale");

        let since = SystemTime::now();
        set_mtime(&estimates, since - Duration::from_secs(1));
        set_mtime(&stale_estimates, since - Duration::from_hours(1));

        let source = FsBenchOutputSource::new(dir.path());
        let harvest = source
            .collect(EngineSystem::Criterion, since)
            .await
            .unwrap();

        let cases = criterion_cases(harvest);
        let benchmarks: Vec<&str> = cases.iter().map(|case| case.benchmark.as_str()).collect();
        assert_eq!(benchmarks, vec!["bm-fresh"]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn criterion_missing_tree_yields_no_cases() {
        let dir = tempdir().unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let harvest = source
            .collect(EngineSystem::Criterion, SystemTime::UNIX_EPOCH)
            .await
            .unwrap();

        assert!(criterion_cases(harvest).is_empty());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn criterion_unreadable_tree_reports_error() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(CRITERION_DIR), "not a directory").unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let error = source
            .collect(EngineSystem::Criterion, SystemTime::UNIX_EPOCH)
            .await
            .unwrap_err();

        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }
}
