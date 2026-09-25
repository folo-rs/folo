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
    /// Returns the output `engine` wrote, gated by `since`.
    ///
    /// `since` is the freshness boundary: `Some(t)` keeps only files modified at or
    /// after `t` (minus a small slack), discarding stale leftovers from an earlier
    /// run in the same tree; `None` disables the gate and admits every matching
    /// file, so a caller that curates the tree itself (such as `import`) harvests
    /// everything present.
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
        since: Option<SystemTime>,
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
        since: Option<SystemTime>,
        reporter: &dyn Reporter,
    ) -> io::Result<Vec<RawSummary>> {
        let threshold = since.map(freshness_threshold);
        let summary_name = OsStr::new(SUMMARY_FILE);

        let root = self.target_root.join(GUNGRAUN_DIR);
        reporter.note_with(|| {
            format!(
                "callgrind: scanning {} for {SUMMARY_FILE} files modified at or after {}",
                root.display(),
                format_threshold(threshold)
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
                    if threshold.is_none_or(|threshold| modified >= threshold) {
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
        since: Option<SystemTime>,
        reporter: &dyn Reporter,
    ) -> io::Result<Vec<RawCriterionCase>> {
        let threshold = since.map(freshness_threshold);
        let benchmark_name = OsStr::new(CRITERION_BENCHMARK_FILE);
        let estimates_name = OsStr::new(CRITERION_ESTIMATES_FILE);
        let new_dir_name = OsStr::new(CRITERION_NEW_DIR);

        let root = self.target_root.join(CRITERION_DIR);
        reporter.note_with(|| format!(
            "criterion: scanning {} for {CRITERION_NEW_DIR}/ cases with {CRITERION_ESTIMATES_FILE} \
             modified at or after {}",
            root.display(),
            format_threshold(threshold)
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
            if threshold.is_none_or(|threshold| modified >= threshold) {
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
        since: Option<SystemTime>,
        reporter: &dyn Reporter,
    ) -> io::Result<Vec<RawOperationFile>> {
        let threshold = since.map(freshness_threshold);
        let json_extension = OsStr::new("json");

        let root = self.target_root.join(engine_dir);
        reporter.note_with(|| {
            format!(
                "{label}: scanning {} for *.json files modified at or after {}",
                root.display(),
                format_threshold(threshold)
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
            if threshold.is_none_or(|threshold| modified >= threshold) {
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
        since: Option<SystemTime>,
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

/// The effective mtime cutoff for a freshness boundary: `since` minus
/// [`MTIME_SLACK`] (saturating at the epoch).
///
/// Callers apply it per boundary via [`Option::map`], so a disabled gate (`None`)
/// stays `None` and admits every candidate file.
fn freshness_threshold(since: SystemTime) -> SystemTime {
    let since_epoch = since
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    SystemTime::UNIX_EPOCH
        .checked_add(since_epoch.saturating_sub(MTIME_SLACK))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Renders a freshness threshold for a diagnostic note.
///
/// A disabled gate (`None`) is described in words, since there is no instant to
/// format.
fn format_threshold(threshold: Option<SystemTime>) -> String {
    threshold.map_or_else(
        || "the beginning of time (freshness gate disabled)".to_owned(),
        format_mtime,
    )
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

    use super::*;

    #[test]
    fn format_mtime_renders_a_known_instant_as_rfc3339() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(3);
        assert_eq!(format_mtime(time), "1970-01-01T00:00:03Z");
    }

    #[test]
    fn format_threshold_describes_a_disabled_gate_in_words() {
        assert_eq!(
            format_threshold(None),
            "the beginning of time (freshness gate disabled)"
        );
    }

    #[test]
    fn format_threshold_renders_an_enabled_gate_as_its_instant() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(3);
        assert_eq!(format_threshold(Some(time)), "1970-01-01T00:00:03Z");
    }

    #[test]
    fn freshness_threshold_subtracts_the_slack() {
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(50);
        assert_eq!(
            freshness_threshold(since),
            SystemTime::UNIX_EPOCH + Duration::from_secs(50) - MTIME_SLACK
        );
    }

    #[test]
    fn freshness_threshold_saturates_at_the_epoch_for_near_epoch_boundaries() {
        // A boundary within MTIME_SLACK of the epoch must not push the cutoff
        // forward: saturating at the epoch keeps every old file admitted.
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        assert_eq!(freshness_threshold(since), SystemTime::UNIX_EPOCH);
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
}
