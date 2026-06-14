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

use crate::bench::{GUNGRAUN_DIR, SUMMARY_FILE};
use crate::comparability::EngineSystem;

/// Tolerance subtracted from the run-start boundary before comparing file
/// modification times, absorbing coarse filesystem mtime granularity so a summary
/// written moments after the run started is never mistaken for a stale one.
const MTIME_SLACK: Duration = Duration::from_secs(2);

/// One harvested summary file: its path and raw contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawSummary {
    /// Filesystem path the summary was read from.
    pub(crate) path: PathBuf,
    /// Raw file contents (engine-specific JSON).
    pub(crate) content: String,
}

/// Collects the summaries an engine produced during a run.
pub(crate) trait BenchOutputSource {
    /// Returns every summary file `engine` wrote at or after `since`.
    ///
    /// # Errors
    ///
    /// Returns an error if the output tree cannot be read.
    fn collect(
        &self,
        engine: EngineSystem,
        since: SystemTime,
    ) -> impl Future<Output = io::Result<Vec<RawSummary>>>;
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
}

impl BenchOutputSource for FsBenchOutputSource {
    async fn collect(
        &self,
        engine: EngineSystem,
        since: SystemTime,
    ) -> io::Result<Vec<RawSummary>> {
        match engine {
            EngineSystem::Callgrind => self.collect_callgrind(since).await,
            EngineSystem::Criterion => Ok(Vec::new()),
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

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn collects_fresh_summaries_recursively() {
        let dir = tempdir().unwrap();
        write_summary(dir.path(), "group_a/summary.json", "a");
        write_summary(dir.path(), "group_b/nested/summary.json", "b");
        write_summary(dir.path(), "group_a/other.json", "ignored");

        let source = FsBenchOutputSource::new(dir.path());
        let since = SystemTime::now() - Duration::from_mins(1);
        let summaries = source
            .collect(EngineSystem::Callgrind, since)
            .await
            .unwrap();

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
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        let source = FsBenchOutputSource::new(dir.path());
        let summaries = source
            .collect(EngineSystem::Callgrind, SystemTime::now())
            .await
            .unwrap();

        assert!(summaries.is_empty(), "stale summary should be excluded");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn missing_output_tree_yields_no_summaries() {
        let dir = tempdir().unwrap();
        let source = FsBenchOutputSource::new(dir.path());

        let summaries = source
            .collect(EngineSystem::Callgrind, SystemTime::now())
            .await
            .unwrap();

        assert!(summaries.is_empty());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn criterion_collects_nothing_in_this_iteration() {
        let dir = tempdir().unwrap();
        write_summary(dir.path(), "group/summary.json", "a");
        let source = FsBenchOutputSource::new(dir.path());

        let summaries = source
            .collect(EngineSystem::Criterion, SystemTime::UNIX_EPOCH)
            .await
            .unwrap();

        assert!(summaries.is_empty());
    }
}
