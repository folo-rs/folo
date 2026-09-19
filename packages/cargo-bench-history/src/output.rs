//! The filesystem edge that writes the per-format reports the reporting commands
//! (`analyze`, `list`, `prune`, `examine`) render.
//!
//! `cbh_analyze` renders each requested format into a [`RenderedReports`] and
//! returns it; this module writes the `Some` fields to the paths the user gave.
//! Text is the default and goes to standard output (the caller prints it), so it is
//! never written here; `--markdown <path>` and `--json <path>` each write that
//! format to a file, and `analyze` additionally offers `--markdown-summary <path>`
//! plus a one-line `--outcome <path>`.
//! The file writes go through the [`OutputWriter`] port (mirroring the `ConfigWriter`
//! used by `install`) so the write path stays filesystem-agnostic: production uses
//! [`TokioOutputWriter`], while tests drive an in-memory fake.
//!
//! A relative `--markdown`/`--markdown-summary`/`--json`/`--outcome` path resolves against the
//! working directory (the same base as `--config`), so the resolution happens at the
//! IO edge inside [`TokioOutputWriter`].

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use cbh_analyze::RenderedReports;
use cbh_config::rebase;
use cbh_diag::{Reporter, ReporterExt};
use ohno::AppError;

use crate::errors::{ConflictingReportDestinationsError, WriteReportFailedError};
use crate::output_destination::same_destination;

/// Checks report destinations and writes reports, overwriting existing files.
///
/// This is the filesystem edge of the per-format output model: `cbh_analyze` renders
/// strings and the binary hands them here, so the report rendering stays Miri-safe
/// and an in-memory fake can stand in under test.
pub(crate) trait OutputWriter {
    /// Compares destinations as the filesystem would resolve them, without writing reports
    /// or creating their parent directories.
    fn same_destination(&self, left: &Path, right: &Path)
    -> impl Future<Output = io::Result<bool>>;

    /// Writes `contents` to `path`, creating parent directories as needed and
    /// replacing any existing file (a re-run refreshes the report in place).
    fn write(&self, path: &Path, contents: &str) -> impl Future<Output = io::Result<()>>;
}

/// The production [`OutputWriter`], backed by `tokio::fs`.
///
/// A relative destination resolves against `base` (the workspace directory, which
/// is the working directory in production), so `--markdown report.md` lands beside
/// the other working-directory-relative paths the tool accepts.
#[derive(Clone, Debug)]
pub(crate) struct TokioOutputWriter {
    /// The directory a relative destination path is resolved against.
    base: PathBuf,
}

impl TokioOutputWriter {
    /// Creates a writer that resolves relative paths against `base`.
    #[must_use]
    pub(crate) fn new(base: PathBuf) -> Self {
        Self { base }
    }
}

impl OutputWriter for TokioOutputWriter {
    // Filesystem identity is exercised by native command integration tests.
    #[cfg_attr(test, mutants::skip)]
    async fn same_destination(&self, left: &Path, right: &Path) -> io::Result<bool> {
        let left = rebase(&self.base, left.to_path_buf());
        let right = rebase(&self.base, right.to_path_buf());
        tokio::task::spawn_blocking(move || same_destination(&left, &right))
            .await
            .map_err(io::Error::other)?
    }

    // Filesystem writes are exercised by native command integration tests.
    #[cfg_attr(test, mutants::skip)]
    async fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
        let resolved = rebase(&self.base, path.to_path_buf());
        if let Some(parent) = resolved.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        // `write` truncates an existing file, so re-running analysis refreshes the
        // report in place rather than appending to or failing on a stale one.
        tokio::fs::write(&resolved, contents.as_bytes()).await
    }
}

/// Writes each rendered report to its requested destination path.
///
/// The `Some`-ness of each [`RenderedReports`] field is the single source of truth
/// for what gets written for the optional report formats: `cbh_analyze` renders a
/// format exactly when the user requested its path, so a rendered field and its
/// destination path always agree. The outcome is different: analysis always computes
/// it for the in-process caller and only writes it when `--outcome` supplied a path.
/// Each file write is announced on the verbose trail with its path and size, so a
/// `--verbose` run records exactly what landed where.
///
/// # Errors
///
/// Returns a [`ConflictingReportDestinationsError`] if reports share a destination,
/// before any report is written. Returns a [`WriteReportFailedError`] if checking
/// or writing a requested file fails.
pub(crate) async fn write_reports<W: OutputWriter>(
    writer: &W,
    reporter: &dyn Reporter,
    markdown: Option<&Path>,
    json: Option<&Path>,
    markdown_summary: Option<&Path>,
    outcome: Option<&Path>,
    rendered: &RenderedReports,
) -> Result<(), AppError> {
    debug_assert_eq!(
        markdown.is_some(),
        rendered.markdown.is_some(),
        "a --markdown path and a rendered Markdown report must accompany each other"
    );
    debug_assert_eq!(
        json.is_some(),
        rendered.json.is_some(),
        "a --json path and a rendered JSON report must accompany each other"
    );
    debug_assert_eq!(
        markdown_summary.is_some(),
        rendered.markdown_summary.is_some(),
        "a --markdown-summary path and a rendered summary must accompany each other"
    );
    let outcome_contents = outcome.map(|_| {
        rendered
            .outcome
            .expect("an --outcome path is valid only for analyze, which always has an outcome")
            .as_str()
    });
    let reports = [
        (markdown, rendered.markdown.as_deref(), "Markdown"),
        (json, rendered.json.as_deref(), "JSON"),
        (
            markdown_summary,
            rendered.markdown_summary.as_deref(),
            "Markdown summary",
        ),
        (outcome, outcome_contents, "analysis outcome"),
    ];
    let reports: Vec<_> = reports
        .into_iter()
        .filter_map(|(path, contents, label)| {
            path.zip(contents)
                .map(|(path, contents)| (path, contents, label))
        })
        .collect();

    // Check the entire set before the first write: a collision between the last reports
    // must also leave earlier, unrelated destinations untouched.
    for (index, &(path, _, label)) in reports.iter().enumerate() {
        for &(earlier_path, _, earlier_label) in reports.iter().take(index) {
            if path == earlier_path
                || writer
                    .same_destination(earlier_path, path)
                    .await
                    .map_err(|error| WriteReportFailedError::caused_by(label, path, error))?
            {
                return Err(ConflictingReportDestinationsError::new(
                    earlier_label,
                    earlier_path,
                    label,
                    path,
                )
                .into());
            }
        }
    }
    for (path, contents, label) in reports {
        write_report(writer, reporter, path, contents, label).await?;
    }
    Ok(())
}

/// Writes one rendered report to `path` and records the result on the verbose
/// trail.
async fn write_report<W: OutputWriter>(
    writer: &W,
    reporter: &dyn Reporter,
    path: &Path,
    contents: &str,
    label: &str,
) -> Result<(), AppError> {
    writer
        .write(path, contents)
        .await
        .map_err(|error| WriteReportFailedError::caused_by(label, path, error))?;
    reporter.note_with(|| {
        format!(
            "wrote the {label} report to {} ({})",
            path.display(),
            cbh_diag::count_noun(contents.len(), "byte")
        )
    });
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod fake {
    use std::collections::HashMap;
    use std::future::{Future, ready};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::{OutputWriter, io};

    /// An in-memory [`OutputWriter`] that records written files without touching
    /// the filesystem, so write-path tests run under Miri.
    ///
    /// A later write to the same path overwrites the recorded contents, mirroring
    /// the real writer.
    #[derive(Debug, Default)]
    pub(crate) struct MemoryOutputWriter {
        files: Mutex<HashMap<PathBuf, String>>,
        aliases: HashMap<PathBuf, PathBuf>,
    }

    impl MemoryOutputWriter {
        /// An empty writer.
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// The contents recorded for `path`, if any.
        pub(crate) fn written(&self, path: &Path) -> Option<String> {
            self.files.lock().unwrap().get(path).cloned()
        }

        pub(crate) fn alias(&mut self, path: &Path, destination: &Path) {
            self.aliases
                .insert(path.to_path_buf(), destination.to_path_buf());
        }
    }

    impl OutputWriter for MemoryOutputWriter {
        fn same_destination(
            &self,
            left: &Path,
            right: &Path,
        ) -> impl Future<Output = io::Result<bool>> {
            ready(Ok(self.aliases.get(left).map_or(left, PathBuf::as_path)
                == self.aliases.get(right).map_or(right, PathBuf::as_path)))
        }

        fn write(&self, path: &Path, contents: &str) -> impl Future<Output = io::Result<()>> {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), contents.to_owned());
            ready(Ok(()))
        }
    }

    /// An [`OutputWriter`] whose inspections and writes fail, exercising error
    /// propagation under Miri without touching the filesystem.
    #[derive(Debug, Default)]
    pub(crate) struct FailingOutputWriter;

    impl OutputWriter for FailingOutputWriter {
        fn same_destination(
            &self,
            _left: &Path,
            _right: &Path,
        ) -> impl Future<Output = io::Result<bool>> {
            ready(Err(io::ErrorKind::PermissionDenied.into()))
        }

        fn write(&self, _path: &Path, _contents: &str) -> impl Future<Output = io::Result<()>> {
            ready(Err(io::Error::other("write refused")))
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        reason = "fixed-size report fixtures use known indices"
    )]

    use cbh_diag::RecordingReporter;
    use futures::executor::block_on;

    use super::fake::{FailingOutputWriter, MemoryOutputWriter};
    use super::*;
    use crate::AnalysisOutcome;

    fn all_formats() -> RenderedReports {
        RenderedReports {
            outcome: Some(AnalysisOutcome::Clean),
            text: None,
            markdown: Some("Markdown".to_owned()),
            json: Some("Json".to_owned()),
            markdown_summary: Some("Summary".to_owned()),
        }
    }

    fn assert_collision(paths: [&Path; 4], first: usize, second: usize) {
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();
        for path in paths {
            block_on(writer.write(path, "existing contents")).unwrap();
        }
        let error = block_on(write_reports(
            &writer,
            &reporter,
            Some(paths[0]),
            Some(paths[1]),
            Some(paths[2]),
            Some(paths[3]),
            &all_formats(),
        ))
        .unwrap_err();

        let collision = error
            .find_source::<ConflictingReportDestinationsError>()
            .unwrap();
        let labels = ["Markdown", "JSON", "Markdown summary", "analysis outcome"];
        assert_eq!(collision.first_label, labels[first]);
        assert_eq!(collision.second_label, labels[second]);
        assert_eq!(collision.first_path, paths[first]);
        assert_eq!(collision.second_path, paths[second]);
        for path in paths {
            assert_eq!(writer.written(path).as_deref(), Some("existing contents"));
        }
        assert!(reporter.notes().is_empty());
    }

    #[test]
    fn write_reports_rejects_every_format_pair_without_writing() {
        let paths = [
            Path::new("report.md"),
            Path::new("report.json"),
            Path::new("summary.md"),
            Path::new("outcome.txt"),
        ];
        for first in 0..paths.len() {
            for second in (first + 1)..paths.len() {
                let mut paths = paths;
                paths[second] = paths[first];
                assert_collision(paths, first, second);
            }
        }
    }

    #[test]
    fn write_reports_rejects_filesystem_aliases_before_earlier_writes() {
        let mut writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();
        let markdown = Path::new("report.md");
        let json = Path::new("report.json");
        let outcome = Path::new("alias.json");
        writer.alias(outcome, json);
        block_on(writer.write(json, "existing JSON")).unwrap();
        let rendered = RenderedReports {
            markdown_summary: None,
            ..all_formats()
        };

        let error = block_on(write_reports(
            &writer,
            &reporter,
            Some(markdown),
            Some(json),
            None,
            Some(outcome),
            &rendered,
        ))
        .unwrap_err();

        let collision = error
            .find_source::<ConflictingReportDestinationsError>()
            .unwrap();
        assert_eq!(collision.first_path, json);
        assert_eq!(collision.second_path, outcome);
        assert_eq!(writer.written(json).as_deref(), Some("existing JSON"));
        assert!(writer.written(markdown).is_none());
        assert!(writer.written(outcome).is_none());
        assert!(reporter.notes().is_empty());
    }

    #[test]
    fn write_reports_stops_on_preflight_io_failure() {
        let error = block_on(write_reports(
            &FailingOutputWriter,
            &RecordingReporter::new(),
            Some(Path::new("report.md")),
            Some(Path::new("report.json")),
            Some(Path::new("summary.md")),
            Some(Path::new("outcome.txt")),
            &all_formats(),
        ))
        .unwrap_err();

        assert!(error.find_source::<WriteReportFailedError>().is_some());
        assert_eq!(
            error.find_source::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn write_reports_writes_all_distinct_formats() {
        let writer = MemoryOutputWriter::new();
        let paths = [
            Path::new("report.md"),
            Path::new("report.json"),
            Path::new("summary.md"),
            Path::new("outcome.txt"),
        ];
        block_on(write_reports(
            &writer,
            &RecordingReporter::new(),
            Some(paths[0]),
            Some(paths[1]),
            Some(paths[2]),
            Some(paths[3]),
            &all_formats(),
        ))
        .unwrap();

        for (path, contents) in paths
            .into_iter()
            .zip(["Markdown", "Json", "Summary", "clean"])
        {
            assert_eq!(writer.written(path).as_deref(), Some(contents));
        }
    }

    #[test]
    fn write_reports_writes_both_files_and_still_announces_them() {
        let markdown = PathBuf::from("report.md");
        let json = PathBuf::from("report.json");
        let rendered = RenderedReports {
            outcome: None,
            text: Some("Text".to_owned()),
            markdown: Some("Markdown".to_owned()),
            json: Some("Json".to_owned()),
            markdown_summary: None,
        };
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        block_on(write_reports(
            &writer,
            &reporter,
            Some(&markdown),
            Some(&json),
            None,
            None,
            &rendered,
        ))
        .unwrap();

        assert_eq!(writer.written(&markdown).as_deref(), Some("Markdown"));
        assert_eq!(writer.written(&json).as_deref(), Some("Json"));
        // Each written file is announced on the verbose trail.
        assert!(
            reporter.contains("wrote the Markdown report"),
            "{:?}",
            reporter.notes()
        );
        assert!(
            reporter.contains("wrote the JSON report"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn write_reports_writes_nothing_for_a_text_only_render() {
        // The default text-only render carries no file destinations, so nothing is
        // written and no note fires.
        let rendered = RenderedReports {
            text: Some("Text".to_owned()),
            ..RenderedReports::default()
        };
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        block_on(write_reports(
            &writer, &reporter, None, None, None, None, &rendered,
        ))
        .unwrap();

        assert!(reporter.notes().is_empty(), "{:?}", reporter.notes());
    }

    #[test]
    fn write_reports_writes_the_summary_and_announces_it() {
        let summary = PathBuf::from("summary.md");
        let rendered = RenderedReports {
            markdown_summary: Some("SUMMARY".to_owned()),
            ..RenderedReports::default()
        };
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        block_on(write_reports(
            &writer,
            &reporter,
            None,
            None,
            Some(&summary),
            None,
            &rendered,
        ))
        .unwrap();

        assert_eq!(writer.written(&summary).as_deref(), Some("SUMMARY"));
        assert!(
            reporter.contains("wrote the Markdown summary report"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn write_reports_writes_the_analysis_outcome_and_announces_it() {
        let outcome = PathBuf::from("outcome.txt");
        let rendered = RenderedReports {
            outcome: Some(AnalysisOutcome::InsufficientBaseline),
            ..RenderedReports::default()
        };
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        block_on(write_reports(
            &writer,
            &reporter,
            None,
            None,
            None,
            Some(&outcome),
            &rendered,
        ))
        .unwrap();

        assert_eq!(
            writer.written(&outcome).as_deref(),
            Some("insufficient_baseline")
        );
        assert!(
            reporter.contains("wrote the analysis outcome report"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn write_reports_names_the_report_that_could_not_be_written() {
        let json = PathBuf::from("report.json");
        let rendered = RenderedReports {
            json: Some("Json".to_owned()),
            ..RenderedReports::default()
        };
        let writer = FailingOutputWriter;
        let reporter = RecordingReporter::new();

        let error = block_on(write_reports(
            &writer,
            &reporter,
            None,
            Some(&json),
            None,
            None,
            &rendered,
        ))
        .unwrap_err();
        let write_error = error.find_source::<WriteReportFailedError>().unwrap();
        assert_eq!(write_error.label, "JSON");
        assert_eq!(write_error.path, json);
        assert!(error.find_source::<io::Error>().is_some());
    }
}
