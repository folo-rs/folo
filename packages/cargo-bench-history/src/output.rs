//! The per-format output model shared by the reporting commands (`analyze`,
//! `list`, `prune`): one analysis pass renders any combination of the text,
//! Markdown, and JSON reports.
//!
//! Text is the default and goes to standard output; `--no-text` suppresses it.
//! `--markdown <path>` and `--json <path>` each write that format to a file. The
//! file writes go through the [`OutputWriter`] port (mirroring the [`ConfigWriter`]
//! used by `install`) so the orchestrators stay storage- and filesystem-agnostic:
//! production uses [`TokioOutputWriter`], while tests drive an in-memory fake.
//!
//! `analyze` additionally offers `--markdown-summary <path>`: a condensed Markdown
//! report carrying only the most significant findings, so a large analysis still
//! fits within a GitHub issue body. It is analyze-only (the other commands do not
//! rank findings), so it is not one of the three shared formats — it resolves and
//! writes through [`OutputSelection::resolve_analyze`] and [`emit_markdown_summary`]
//! rather than the shared [`emit`].
//!
//! A relative `--markdown`/`--markdown-summary`/`--json` path resolves against the
//! working directory (the same base as `--config`), so the resolution happens at the
//! IO edge inside [`TokioOutputWriter`].
//!
//! [`ConfigWriter`]: crate::config_writer::ConfigWriter

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use cargo_bench_history_core::analyze::ReportFormat;
use cbh_diag::{Reporter, ReporterExt};

use crate::RunError;
use crate::wiring::rebase;

/// Which report formats a single analysis pass should emit.
///
/// Built once, early in each orchestrator, so the "nothing to emit" case (text
/// suppressed and no file requested) fails before any expensive work.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputSelection<'a> {
    /// Whether the text report to standard output is suppressed (`--no-text`).
    no_text: bool,
    /// Where to write the Markdown report, if requested (`--markdown <path>`).
    markdown: Option<&'a Path>,
    /// Where to write the JSON report, if requested (`--json <path>`).
    json: Option<&'a Path>,
    /// Where to write the condensed Markdown summary, if requested
    /// (`--markdown-summary <path>`). Only `analyze` sets this; the other reporting
    /// commands leave it `None` (they do not rank findings).
    markdown_summary: Option<&'a Path>,
}

impl<'a> OutputSelection<'a> {
    /// Resolves and validates the requested outputs for the commands that emit only
    /// the three shared formats (`list`, `prune`, `examine`).
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Analyze`] when nothing would be emitted — `--no-text`
    /// suppresses the only default output and neither file format was requested —
    /// so a run that would produce nothing is rejected up front rather than
    /// completing silently.
    pub(crate) fn resolve(
        no_text: bool,
        markdown: Option<&'a Path>,
        json: Option<&'a Path>,
    ) -> Result<Self, RunError> {
        if no_text && markdown.is_none() && json.is_none() {
            return Err(RunError::Analyze {
                message: "no output selected: --no-text suppresses the text report, so request at \
                          least one of --markdown <path> or --json <path>"
                    .to_owned(),
            });
        }
        Ok(Self {
            no_text,
            markdown,
            json,
            markdown_summary: None,
        })
    }

    /// Resolves and validates the requested outputs for `analyze`, which additionally
    /// offers `--markdown-summary <path>`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Analyze`] when nothing would be emitted — `--no-text`
    /// suppresses the text report and none of `--markdown`, `--markdown-summary`, or
    /// `--json` was requested — so a run that would produce nothing is rejected up
    /// front rather than completing silently.
    pub(crate) fn resolve_analyze(
        no_text: bool,
        markdown: Option<&'a Path>,
        json: Option<&'a Path>,
        markdown_summary: Option<&'a Path>,
    ) -> Result<Self, RunError> {
        if no_text && markdown.is_none() && json.is_none() && markdown_summary.is_none() {
            return Err(RunError::Analyze {
                message: "no output selected: --no-text suppresses the text report, so request at \
                          least one of --markdown <path>, --markdown-summary <path>, or --json \
                          <path>"
                    .to_owned(),
            });
        }
        Ok(Self {
            no_text,
            markdown,
            json,
            markdown_summary,
        })
    }
}

/// Writes a rendered report to a destination path, overwriting any existing file.
///
/// This is the filesystem edge of the per-format output model: the pure
/// orchestrators render strings and hand them here, so the report rendering stays
/// Miri-safe and an in-memory fake can stand in under test.
pub(crate) trait OutputWriter {
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
    pub(crate) fn new(base: PathBuf) -> Self {
        Self { base }
    }
}

impl OutputWriter for TokioOutputWriter {
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

/// Emits every requested report format from one rendered pass and returns the text
/// report destined for standard output (empty when `--no-text` suppressed it).
///
/// `render` produces the report for a given format; it is invoked once per
/// requested format (and once more for the text report unless suppressed), so a
/// single analysis backs all outputs. Each file write is announced on the verbose
/// trail with its path and size, so a `--verbose` run records exactly what landed
/// where.
///
/// # Errors
///
/// Returns [`RunError::Io`] if writing a requested file fails.
pub(crate) async fn emit<W, F>(
    selection: &OutputSelection<'_>,
    writer: &W,
    reporter: &dyn Reporter,
    render: F,
) -> Result<String, RunError>
where
    W: OutputWriter,
    F: Fn(ReportFormat) -> String,
{
    if let Some(path) = selection.markdown {
        let contents = render(ReportFormat::Markdown);
        write_report(writer, reporter, path, &contents, "Markdown").await?;
    }
    if let Some(path) = selection.json {
        let contents = render(ReportFormat::Json);
        write_report(writer, reporter, path, &contents, "JSON").await?;
    }
    Ok(if selection.no_text {
        String::new()
    } else {
        render(ReportFormat::Text)
    })
}

/// Writes the condensed Markdown summary to its requested path, if one was selected.
///
/// This is `analyze`'s companion to [`emit`]: the top-N summary is not one of the
/// three shared report formats (it is a truncated, derived view), so it renders
/// through its own `render_summary` closure and writes via the same [`OutputWriter`]
/// edge, announcing the result on the verbose trail like every other written report.
/// A no-op when `--markdown-summary` was not requested.
///
/// # Errors
///
/// Returns [`RunError::Io`] if writing the summary file fails.
pub(crate) async fn emit_markdown_summary<W, F>(
    selection: &OutputSelection<'_>,
    writer: &W,
    reporter: &dyn Reporter,
    render_summary: F,
) -> Result<(), RunError>
where
    W: OutputWriter,
    F: FnOnce() -> String,
{
    if let Some(path) = selection.markdown_summary {
        let contents = render_summary();
        write_report(writer, reporter, path, &contents, "Markdown summary").await?;
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
) -> Result<(), RunError> {
    writer.write(path, contents).await.map_err(RunError::Io)?;
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
pub(crate) use fake::MemoryOutputWriter;

#[cfg(test)]
mod fake {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::{OutputWriter, io};

    /// An in-memory [`OutputWriter`] that records written files without touching
    /// the filesystem, so orchestration tests run under Miri. A later write to the
    /// same path overwrites the recorded contents, mirroring the real writer.
    #[derive(Debug, Default)]
    pub(crate) struct MemoryOutputWriter {
        files: Mutex<HashMap<PathBuf, String>>,
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
    }

    impl OutputWriter for MemoryOutputWriter {
        async fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), contents.to_owned());
            Ok(())
        }
    }

    /// An [`OutputWriter`] whose every write fails, so the emit error path is
    /// exercised under Miri without touching the filesystem.
    #[derive(Debug, Default)]
    pub(crate) struct FailingOutputWriter;

    impl OutputWriter for FailingOutputWriter {
        async fn write(&self, _path: &Path, _contents: &str) -> io::Result<()> {
            Err(io::Error::other("write refused"))
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_diag::RecordingReporter;
    use futures::executor::block_on;

    use super::fake::FailingOutputWriter;
    use super::*;

    /// A render stub that names the format it was asked for, so a test can tell
    /// which format backs each written file and the returned text.
    fn render_label(format: ReportFormat) -> String {
        format!("{format:?}")
    }

    #[test]
    fn resolve_rejects_a_run_that_would_emit_nothing() {
        let error = OutputSelection::resolve(true, None, None).unwrap_err();
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("no output selected"), "{message}");
            }
            other => panic!("expected an analyze error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_allows_the_default_text_output() {
        OutputSelection::resolve(false, None, None).unwrap();
    }

    #[test]
    fn resolve_allows_no_text_with_a_requested_file() {
        let json = PathBuf::from("report.json");
        OutputSelection::resolve(true, None, Some(&json)).unwrap();
    }

    #[test]
    fn emit_returns_the_text_report_by_default() {
        let selection = OutputSelection::resolve(false, None, None).unwrap();
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        let text = block_on(emit(&selection, &writer, &reporter, render_label)).unwrap();

        assert_eq!(text, "Text");
    }

    #[test]
    fn emit_suppresses_the_text_report_under_no_text() {
        let json = PathBuf::from("report.json");
        let selection = OutputSelection::resolve(true, None, Some(&json)).unwrap();
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        let text = block_on(emit(&selection, &writer, &reporter, render_label)).unwrap();

        assert!(text.is_empty(), "{text:?}");
        assert_eq!(writer.written(&json).as_deref(), Some("Json"));
    }

    #[test]
    fn emit_writes_both_files_from_one_pass_and_still_returns_text() {
        let markdown = PathBuf::from("report.md");
        let json = PathBuf::from("report.json");
        let selection = OutputSelection::resolve(false, Some(&markdown), Some(&json)).unwrap();
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        let text = block_on(emit(&selection, &writer, &reporter, render_label)).unwrap();

        assert_eq!(text, "Text");
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
    fn emit_maps_a_write_failure_to_an_io_error() {
        let json = PathBuf::from("report.json");
        let selection = OutputSelection::resolve(true, None, Some(&json)).unwrap();
        let writer = FailingOutputWriter;
        let reporter = RecordingReporter::new();

        let error = block_on(emit(&selection, &writer, &reporter, render_label)).unwrap_err();

        assert!(matches!(error, RunError::Io(_)), "{error:?}");
    }

    #[test]
    fn resolve_analyze_allows_only_the_summary() {
        let summary = PathBuf::from("summary.md");
        OutputSelection::resolve_analyze(true, None, None, Some(&summary)).unwrap();
    }

    #[test]
    fn resolve_analyze_rejects_a_run_that_would_emit_nothing() {
        let error = OutputSelection::resolve_analyze(true, None, None, None).unwrap_err();
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("no output selected"), "{message}");
                // The analyze variant names its extra format in the guidance.
                assert!(message.contains("--markdown-summary"), "{message}");
            }
            other => panic!("expected an analyze error, got {other:?}"),
        }
    }

    #[test]
    fn emit_markdown_summary_writes_the_file_and_announces_it() {
        let summary = PathBuf::from("summary.md");
        let selection = OutputSelection::resolve_analyze(true, None, None, Some(&summary)).unwrap();
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        block_on(emit_markdown_summary(
            &selection,
            &writer,
            &reporter,
            || "SUMMARY".to_owned(),
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
    fn emit_markdown_summary_is_a_noop_without_a_requested_path() {
        let json = PathBuf::from("report.json");
        // A selection with the summary unset: the renderer must not run and nothing
        // is written or announced.
        let selection = OutputSelection::resolve_analyze(true, None, Some(&json), None).unwrap();
        let writer = MemoryOutputWriter::new();
        let reporter = RecordingReporter::new();

        let mut rendered = false;
        block_on(emit_markdown_summary(
            &selection,
            &writer,
            &reporter,
            || {
                rendered = true;
                "SUMMARY".to_owned()
            },
        ))
        .unwrap();

        assert!(
            !rendered,
            "the summary renderer must not run when no path is set"
        );
        assert!(reporter.notes().is_empty(), "{:?}", reporter.notes());
    }

    #[test]
    fn emit_markdown_summary_maps_a_write_failure_to_an_io_error() {
        let summary = PathBuf::from("summary.md");
        let selection = OutputSelection::resolve_analyze(true, None, None, Some(&summary)).unwrap();
        let writer = FailingOutputWriter;
        let reporter = RecordingReporter::new();

        let error = block_on(emit_markdown_summary(
            &selection,
            &writer,
            &reporter,
            || "SUMMARY".to_owned(),
        ))
        .unwrap_err();

        assert!(matches!(error, RunError::Io(_)), "{error:?}");
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod real_writer_tests {
    use tempfile::tempdir;

    use super::{OutputWriter, TokioOutputWriter};

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn write_creates_missing_parent_directories() {
        let dir = tempdir().unwrap();
        let writer = TokioOutputWriter::new(dir.path().to_path_buf());

        writer
            .write("nested/report.md".as_ref(), "payload")
            .await
            .unwrap();

        let written = std::fs::read_to_string(dir.path().join("nested/report.md")).unwrap();
        assert_eq!(written, "payload");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn write_overwrites_an_existing_file() {
        let dir = tempdir().unwrap();
        let writer = TokioOutputWriter::new(dir.path().to_path_buf());
        writer.write("report.json".as_ref(), "stale").await.unwrap();

        writer.write("report.json".as_ref(), "fresh").await.unwrap();

        let written = std::fs::read_to_string(dir.path().join("report.json")).unwrap();
        assert_eq!(written, "fresh");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn write_resolves_an_absolute_path_without_rebasing() {
        let base = tempdir().unwrap();
        let elsewhere = tempdir().unwrap();
        let writer = TokioOutputWriter::new(base.path().to_path_buf());
        let absolute = elsewhere.path().join("report.json");

        writer.write(&absolute, "payload").await.unwrap();

        assert_eq!(std::fs::read_to_string(&absolute).unwrap(), "payload");
        // The base directory is untouched, confirming the absolute path was not
        // rebased onto it.
        assert!(!base.path().join("report.json").exists());
    }
}
