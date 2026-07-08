//! The benchmark-output port: collecting the machine-readable summary files an
//! engine wrote during a run, filtered to those produced by this run.
//!
//! The real adapter walks the cargo target tree with `tokio::fs`; an in-memory
//! fake (in `#[cfg(test)]`) returns canned summaries so orchestration is testable.

use std::ffi::OsStr;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cbh_diag::{Reporter, ReporterExt, count_noun};
use cbh_model::Engine;
use jiff::Timestamp;

use crate::bench::{
    ALL_THE_TIME_DIR, ALLOC_TRACKER_DIR, CRITERION_BENCHMARK_FILE, CRITERION_DIR,
    CRITERION_ESTIMATES_FILE, CRITERION_NEW_DIR, GUNGRAUN_DIR, SUMMARY_FILE,
};

/// Tolerance subtracted from the run-start boundary before comparing file
/// modification times, absorbing coarse filesystem mtime granularity so a summary
/// written moments after the run started is never mistaken for a stale one.
const MTIME_SLACK: Duration = Duration::from_secs(2);

/// One harvested Callgrind summary file: its path and raw contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawSummary {
    /// Filesystem path the summary was read from.
    pub path: PathBuf,
    /// Raw file contents (engine-specific JSON).
    pub content: String,
}

/// One harvested Criterion result case: the `new/` directory and the raw contents
/// of the `benchmark.json` and `estimates.json` files it pairs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawCriterionCase {
    /// The `new/` directory the case was read from.
    pub dir: PathBuf,
    /// Raw contents of `benchmark.json` (the case identity).
    pub benchmark: String,
    /// Raw contents of `estimates.json` (the statistical estimates).
    pub estimates: String,
}

/// One harvested flat per-operation file: its path and raw contents.
///
/// Used by the `alloc_tracker` and `all_the_time` engines, which each write one JSON
/// file per operation directly under their engine directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawOperationFile {
    /// Filesystem path the file was read from.
    pub path: PathBuf,
    /// Raw file contents (engine-specific JSON).
    pub content: String,
}

/// The output harvested for a run, in the shape each engine produces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Harvest {
    /// Callgrind (Gungraun) summary files.
    Callgrind(Vec<RawSummary>),
    /// Criterion benchmark/estimates pairs.
    Criterion(Vec<RawCriterionCase>),
    /// `alloc_tracker` per-operation files.
    AllocTracker(Vec<RawOperationFile>),
    /// `all_the_time` per-operation files.
    AllTheTime(Vec<RawOperationFile>),
}

/// Collects the output an engine produced during a run.
pub trait BenchOutputSource {
    /// Returns every output `engine` wrote at or after `since`.
    ///
    /// `reporter` receives a diagnostic note for each directory scanned and each
    /// candidate file included or excluded, so a `--verbose` run can explain an
    /// empty harvest.
    ///
    /// # Errors
    ///
    /// Returns an error if the output tree cannot be read.
    fn collect(
        &self,
        engine: Engine,
        since: SystemTime,
        reporter: &dyn Reporter,
    ) -> impl Future<Output = io::Result<Harvest>>;
}

/// The real [`BenchOutputSource`], walking the cargo target tree.
#[derive(Clone, Debug)]
pub struct FsBenchOutputSource {
    target_root: PathBuf,
}

impl FsBenchOutputSource {
    /// Creates a source rooted at the cargo target directory `target_root`.
    #[must_use]
    pub fn new(target_root: impl Into<PathBuf>) -> Self {
        Self {
            target_root: target_root.into(),
        }
    }

    /// Walks `{target_root}/gungraun` for fresh `summary.json` files.
    async fn collect_callgrind(
        &self,
        since: SystemTime,
        reporter: &dyn Reporter,
    ) -> io::Result<Vec<RawSummary>> {
        let threshold = since.checked_sub(MTIME_SLACK).unwrap_or(since);
        let summary_name = OsStr::new(SUMMARY_FILE);

        let root = self.target_root.join(GUNGRAUN_DIR);
        reporter.note_with(|| {
            format!(
                "callgrind: scanning {} for {SUMMARY_FILE} files modified at or after {}",
                root.display(),
                format_mtime(threshold)
            )
        });

        let mut summaries = Vec::new();
        let mut stack = vec![root.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    note_missing_dir(reporter, "callgrind", &dir, &root);
                    continue;
                }
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
                        reporter.note_with(|| format!("callgrind: including {}", path.display()));
                        let content = tokio::fs::read_to_string(&path).await?;
                        summaries.push(RawSummary { path, content });
                    } else {
                        reporter.note_with(|| format!(
                            "callgrind: excluding {} (modified {}, older than the run boundary)",
                            path.display(),
                            format_mtime(modified)
                        ));
                    }
                }
            }
        }

        summaries.sort_by(|left, right| left.path.cmp(&right.path));
        reporter.note_with(|| {
            format!(
                "callgrind: harvested {}",
                count_noun(summaries.len(), "fresh summary file")
            )
        });
        Ok(summaries)
    }

    /// Walks `{target_root}/criterion` for fresh `new/` result directories.
    ///
    /// Criterion stores each benchmark case under `.../<case>/new/` (the most
    /// recent run) alongside a `base/` directory (the previous run); only `new/`
    /// directories holding both `benchmark.json` and `estimates.json` are
    /// harvested, and a case is admitted only when its `estimates.json` is no
    /// older than the run-start boundary. Incomplete pairs are skipped.
    async fn collect_criterion(
        &self,
        since: SystemTime,
        reporter: &dyn Reporter,
    ) -> io::Result<Vec<RawCriterionCase>> {
        let threshold = since.checked_sub(MTIME_SLACK).unwrap_or(since);
        let benchmark_name = OsStr::new(CRITERION_BENCHMARK_FILE);
        let estimates_name = OsStr::new(CRITERION_ESTIMATES_FILE);
        let new_dir_name = OsStr::new(CRITERION_NEW_DIR);

        let root = self.target_root.join(CRITERION_DIR);
        reporter.note_with(|| format!(
            "criterion: scanning {} for {CRITERION_NEW_DIR}/ cases with {CRITERION_ESTIMATES_FILE} \
             modified at or after {}",
            root.display(),
            format_mtime(threshold)
        ));

        let mut cases = Vec::new();
        let mut stack = vec![root.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    note_missing_dir(reporter, "criterion", &dir, &root);
                    continue;
                }
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

            // A complete, fresh case lives in a `new/` directory holding both
            // files. Non-`new/` directories (group nodes, `base/`) are structural,
            // not incomplete cases, so they are skipped without a note.
            if dir.file_name() != Some(new_dir_name) {
                continue;
            }
            let Some(modified) = estimates_mtime.filter(|_| has_benchmark) else {
                reporter.note_with(|| {
                    format!(
                        "criterion: skipping {} (missing {CRITERION_BENCHMARK_FILE} or \
                         {CRITERION_ESTIMATES_FILE})",
                        dir.display()
                    )
                });
                continue;
            };
            if modified >= threshold {
                reporter.note_with(|| format!("criterion: including {}", dir.display()));
                let benchmark =
                    tokio::fs::read_to_string(dir.join(CRITERION_BENCHMARK_FILE)).await?;
                let estimates =
                    tokio::fs::read_to_string(dir.join(CRITERION_ESTIMATES_FILE)).await?;
                cases.push(RawCriterionCase {
                    dir,
                    benchmark,
                    estimates,
                });
            } else {
                reporter.note_with(|| {
                    format!(
                        "criterion: excluding {} (modified {}, older than the run boundary)",
                        dir.display(),
                        format_mtime(modified)
                    )
                });
            }
        }

        cases.sort_by(|left, right| left.dir.cmp(&right.dir));
        reporter.note_with(|| {
            format!(
                "criterion: harvested {}",
                count_noun(cases.len(), "fresh case")
            )
        });
        Ok(cases)
    }

    /// Walks `{target_root}/{engine_dir}` for fresh top-level `*.json` files.
    ///
    /// `alloc_tracker` and `all_the_time` each write one flat JSON file per
    /// operation directly under their engine directory (no nesting), so only the
    /// immediate `*.json` entries no older than the run-start boundary are
    /// harvested. `label` names the engine in diagnostic notes.
    async fn collect_flat(
        &self,
        engine_dir: &str,
        label: &str,
        since: SystemTime,
        reporter: &dyn Reporter,
    ) -> io::Result<Vec<RawOperationFile>> {
        let threshold = since.checked_sub(MTIME_SLACK).unwrap_or(since);
        let json_extension = OsStr::new("json");

        let root = self.target_root.join(engine_dir);
        reporter.note_with(|| {
            format!(
                "{label}: scanning {} for *.json files modified at or after {}",
                root.display(),
                format_mtime(threshold)
            )
        });

        let mut files = Vec::new();
        let mut entries = match tokio::fs::read_dir(&root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                note_missing_dir(reporter, label, &root, &root);
                return Ok(files);
            }
            Err(error) => return Err(error),
        };

        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let path = entry.path();
            if !file_type.is_file() || path.extension() != Some(json_extension) {
                continue;
            }
            let modified = entry.metadata().await?.modified()?;
            if modified >= threshold {
                reporter.note_with(|| format!("{label}: including {}", path.display()));
                let content = tokio::fs::read_to_string(&path).await?;
                files.push(RawOperationFile { path, content });
            } else {
                reporter.note_with(|| {
                    format!(
                        "{label}: excluding {} (modified {}, older than the run boundary)",
                        path.display(),
                        format_mtime(modified)
                    )
                });
            }
        }

        files.sort_by(|left, right| left.path.cmp(&right.path));
        reporter.note_with(|| {
            format!(
                "{label}: harvested {}",
                count_noun(files.len(), "fresh operation file")
            )
        });
        Ok(files)
    }
}

impl BenchOutputSource for FsBenchOutputSource {
    async fn collect(
        &self,
        engine: Engine,
        since: SystemTime,
        reporter: &dyn Reporter,
    ) -> io::Result<Harvest> {
        match engine {
            Engine::Callgrind => Ok(Harvest::Callgrind(
                self.collect_callgrind(since, reporter).await?,
            )),
            Engine::Criterion => Ok(Harvest::Criterion(
                self.collect_criterion(since, reporter).await?,
            )),
            Engine::AllocTracker => Ok(Harvest::AllocTracker(
                self.collect_flat(ALLOC_TRACKER_DIR, "alloc_tracker", since, reporter)
                    .await?,
            )),
            Engine::AllTheTime => Ok(Harvest::AllTheTime(
                self.collect_flat(ALL_THE_TIME_DIR, "all_the_time", since, reporter)
                    .await?,
            )),
        }
    }
}

/// Formats a filesystem modification time for a diagnostic note, falling back to
/// a placeholder for the rare time that falls outside the timestamp range.
fn format_mtime(time: SystemTime) -> String {
    Timestamp::try_from(time)
        .map_or_else(|_| "<out-of-range>".to_owned(), |stamp| stamp.to_string())
}

/// Emits a diagnostic note for a directory that `read_dir` reported as missing.
///
/// A missing engine root is the common cause of an empty harvest (the engine
/// produced nothing), so it is always reported; a missing nested directory is a
/// rare mid-scan race that is only noted in verbose mode.
fn note_missing_dir(reporter: &dyn Reporter, engine: &str, dir: &Path, root: &Path) {
    if dir == root {
        reporter.note_with(|| {
            format!(
                "{engine}: directory {} does not exist; no output was produced here",
                dir.display()
            )
        });
    } else {
        reporter.note_with(|| {
            format!(
                "{engine}: directory {} disappeared during the scan; skipping",
                dir.display()
            )
        });
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use cbh_diag::RecordingReporter;
    use tempfile::tempdir;

    use super::*;

    /// Collects through the source with a recording reporter, returning both the
    /// harvest and the reporter so a test can assert on either. Most tests ignore
    /// the reporter; the verbose-output tests inspect its notes.
    async fn harvest_with(
        source: &FsBenchOutputSource,
        engine: Engine,
        since: SystemTime,
        reporter: &RecordingReporter,
    ) -> io::Result<Harvest> {
        source.collect(engine, since, reporter).await
    }

    /// Collects through the source, discarding the diagnostic notes.
    async fn harvest(
        source: &FsBenchOutputSource,
        engine: Engine,
        since: SystemTime,
    ) -> io::Result<Harvest> {
        harvest_with(source, engine, since, &RecordingReporter::new()).await
    }

    fn write_summary(root: &Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(GUNGRAUN_DIR).join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn write_criterion_file(root: &Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(CRITERION_DIR).join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn set_mtime(path: &Path, when: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn format_mtime_renders_a_known_instant_as_rfc3339() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(3);
        assert_eq!(format_mtime(time), "1970-01-01T00:00:03Z");
    }

    #[test]
    fn note_missing_dir_reports_a_vanished_nested_directory() {
        // A nested directory that disappears mid-scan (dir != root) is a rare race
        // noted only in verbose mode, distinct from a missing engine root.
        let reporter = RecordingReporter::new();
        let root = Path::new("target/criterion");
        let dir = Path::new("target/criterion/group/new");
        note_missing_dir(&reporter, "criterion", dir, root);
        assert!(
            reporter.contains("disappeared during the scan"),
            "{:?}",
            reporter.notes()
        );
        assert!(
            !reporter.contains("does not exist"),
            "{:?}",
            reporter.notes()
        );
    }

    fn callgrind_summaries(harvest: Harvest) -> Vec<RawSummary> {
        match harvest {
            Harvest::Callgrind(summaries) => summaries,
            _ => panic!("expected callgrind harvest"),
        }
    }

    fn criterion_cases(harvest: Harvest) -> Vec<RawCriterionCase> {
        match harvest {
            Harvest::Criterion(cases) => cases,
            _ => panic!("expected criterion harvest"),
        }
    }

    fn operation_files(harvest: Harvest) -> Vec<RawOperationFile> {
        match harvest {
            Harvest::AllocTracker(files) | Harvest::AllTheTime(files) => files,
            _ => panic!("expected a flat per-operation harvest"),
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
        let harvest = harvest(&source, Engine::Callgrind, since).await.unwrap();

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
        let harvest = harvest(&source, Engine::Callgrind, SystemTime::now())
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
        let harvest = harvest(&source, Engine::Callgrind, since).await.unwrap();

        let summaries = callgrind_summaries(harvest);
        let contents: Vec<&str> = summaries.iter().map(|s| s.content.as_str()).collect();
        assert_eq!(contents, vec!["inside"]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn missing_output_tree_yields_no_summaries() {
        let dir = tempdir().unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let harvest = harvest(&source, Engine::Callgrind, SystemTime::now())
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

        let error = harvest(&source, Engine::Callgrind, SystemTime::UNIX_EPOCH)
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
        let harvest = harvest(&source, Engine::Criterion, since).await.unwrap();

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
        let harvest = harvest(&source, Engine::Criterion, since).await.unwrap();

        let cases = criterion_cases(harvest);
        let benchmarks: Vec<&str> = cases.iter().map(|case| case.benchmark.as_str()).collect();
        assert_eq!(benchmarks, vec!["bm-fresh"]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn criterion_missing_tree_yields_no_cases() {
        let dir = tempdir().unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let harvest = harvest(&source, Engine::Criterion, SystemTime::UNIX_EPOCH)
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

        let error = harvest(&source, Engine::Criterion, SystemTime::UNIX_EPOCH)
            .await
            .unwrap_err();

        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    fn write_operation_file(root: &Path, engine_dir: &str, name: &str, content: &str) -> PathBuf {
        let path = root.join(engine_dir).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn flat_engine_collects_fresh_top_level_json_only() {
        let dir = tempdir().unwrap();
        // Two fresh operation files plus a non-JSON sibling and a nested directory,
        // neither of which is a flat per-operation file.
        write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "allocate_vec.json", "a");
        write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "grow_map.json", "b");
        write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "notes.txt", "ignore me");
        write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "nested/inner.json", "deep");

        let source = FsBenchOutputSource::new(dir.path());
        let since = SystemTime::now() - Duration::from_mins(1);
        let harvest = harvest(&source, Engine::AllocTracker, since).await.unwrap();

        let files = operation_files(harvest);
        let contents: Vec<&str> = files.iter().map(|file| file.content.as_str()).collect();
        assert_eq!(
            contents,
            vec!["a", "b"],
            "only fresh top-level *.json files should be harvested, sorted by path"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn flat_engine_excludes_files_older_than_the_boundary() {
        let dir = tempdir().unwrap();
        let fresh = write_operation_file(dir.path(), ALL_THE_TIME_DIR, "read_cell.json", "fresh");
        let stale = write_operation_file(dir.path(), ALL_THE_TIME_DIR, "write_cell.json", "stale");

        let since = SystemTime::now();
        set_mtime(&fresh, since - Duration::from_secs(1));
        set_mtime(&stale, since - Duration::from_hours(1));

        let source = FsBenchOutputSource::new(dir.path());
        let harvest = harvest(&source, Engine::AllTheTime, since).await.unwrap();

        let files = operation_files(harvest);
        let contents: Vec<&str> = files.iter().map(|file| file.content.as_str()).collect();
        assert_eq!(contents, vec!["fresh"]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn flat_engine_missing_tree_yields_no_files() {
        let dir = tempdir().unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let harvest = harvest(&source, Engine::AllocTracker, SystemTime::UNIX_EPOCH)
            .await
            .unwrap();

        assert!(operation_files(harvest).is_empty());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn flat_engine_unreadable_tree_reports_error() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(ALLOC_TRACKER_DIR), "not a directory").unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let error = harvest(&source, Engine::AllocTracker, SystemTime::UNIX_EPOCH)
            .await
            .unwrap_err();

        assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn verbose_notes_report_a_missing_engine_directory() {
        let dir = tempdir().unwrap();
        let source = FsBenchOutputSource::new(dir.path());
        let reporter = RecordingReporter::new();

        harvest_with(&source, Engine::Criterion, SystemTime::now(), &reporter)
            .await
            .unwrap();

        assert!(
            reporter.contains("does not exist"),
            "an absent engine tree must be reported: {:?}",
            reporter.notes()
        );
        assert!(reporter.contains("harvested 0 fresh cases"));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn verbose_notes_distinguish_included_and_excluded_cases() {
        let dir = tempdir().unwrap();
        let fresh = write_criterion_file(dir.path(), "grp/fresh/now/new/estimates.json", "est");
        write_criterion_file(dir.path(), "grp/fresh/now/new/benchmark.json", "bm");
        let stale = write_criterion_file(dir.path(), "grp/stale/now/new/estimates.json", "est");
        write_criterion_file(dir.path(), "grp/stale/now/new/benchmark.json", "bm");

        let since = SystemTime::now();
        set_mtime(&fresh, since - Duration::from_secs(1));
        set_mtime(&stale, since - Duration::from_hours(1));

        let source = FsBenchOutputSource::new(dir.path());
        let reporter = RecordingReporter::new();
        harvest_with(&source, Engine::Criterion, since, &reporter)
            .await
            .unwrap();

        let notes = reporter.notes();
        assert!(
            notes
                .iter()
                .any(|n| n.contains("including") && n.contains("fresh")),
            "the fresh case must be reported as included: {notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|n| n.contains("excluding") && n.contains("stale")),
            "the stale case must be reported as excluded: {notes:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn verbose_notes_report_an_incomplete_criterion_case() {
        let dir = tempdir().unwrap();
        // A `new/` directory with only benchmark.json is an incomplete case.
        write_criterion_file(dir.path(), "grp/partial/now/new/benchmark.json", "bm");

        let source = FsBenchOutputSource::new(dir.path());
        let reporter = RecordingReporter::new();
        let harvest = harvest_with(
            &source,
            Engine::Criterion,
            SystemTime::UNIX_EPOCH,
            &reporter,
        )
        .await
        .unwrap();

        assert!(criterion_cases(harvest).is_empty());
        assert!(
            reporter
                .notes()
                .iter()
                .any(|n| n.contains("skipping") && n.contains("partial")),
            "an incomplete case must be reported as skipped: {:?}",
            reporter.notes()
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn verbose_notes_report_included_callgrind_summaries() {
        let dir = tempdir().unwrap();
        write_summary(dir.path(), "group/summary.json", "s");

        let source = FsBenchOutputSource::new(dir.path());
        let since = SystemTime::now() - Duration::from_mins(1);
        let reporter = RecordingReporter::new();
        harvest_with(&source, Engine::Callgrind, since, &reporter)
            .await
            .unwrap();

        assert!(
            reporter
                .notes()
                .iter()
                .any(|n| n.contains("including") && n.contains("summary.json")),
            "a fresh summary must be reported as included: {:?}",
            reporter.notes()
        );
        assert!(reporter.contains("harvested 1 fresh summary file"));
    }
}
