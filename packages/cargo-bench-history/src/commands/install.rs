//! The `install` command: write a starter `.cargo/bench_history.toml` if one is
//! absent, then point the user at it. An existing file is never overwritten.

use std::path::Path;

use cbh_config::default_template;
use cbh_diag::{Reporter, ReporterExt, StderrReporter};

use crate::config_writer::{ConfigWriter, TokioConfigWriter};
use crate::wiring::resolve_config_path;
use crate::{InstallOptions, RunError, RunOutcome};

/// Executes the `install` command against the real filesystem.
///
/// # Errors
///
/// Returns [`RunError::Io`] if the configuration file cannot be written.
pub(crate) async fn execute(
    options: &InstallOptions,
    workspace_dir: &Path,
) -> Result<RunOutcome, RunError> {
    let writer = TokioConfigWriter;
    let reporter = StderrReporter::new(options.verbose);
    execute_install(options, workspace_dir, &writer, &reporter).await
}

/// The filesystem-agnostic orchestration, generic over the [`ConfigWriter`] port
/// so tests can drive it with an in-memory fake. The configuration path is
/// resolved relative to `workspace_dir` when not given explicitly.
async fn execute_install<W: ConfigWriter>(
    options: &InstallOptions,
    workspace_dir: &Path,
    writer: &W,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError> {
    let path = resolve_config_path(workspace_dir, options.config_path.as_deref());

    reporter.note_with(|| {
        format!(
            "writing a starter configuration to {} (only if absent)",
            path.display()
        )
    });
    let written = writer.write_new(&path, default_template()).await?;
    if written {
        reporter.note_with(|| "configuration did not exist; wrote the starter template".to_owned());
    } else {
        reporter.note_with(|| "configuration already exists; left it unchanged".to_owned());
    }

    Ok(RunOutcome::Completed {
        message: install_message(&path, written),
    })
}

/// Builds the human-readable summary for an `install` run.
fn install_message(path: &Path, written: bool) -> String {
    let display = path.display();
    if written {
        format!(
            "Wrote a starter configuration to {display}.\n\
             Next steps:\n\
             - For local storage, no configuration is needed: pass `--local=<path>` (or set CARGO_BENCH_HISTORY_STORAGE and pass a bare `--local`) on any command.\n\
             - For cloud storage, edit the file to configure one [storage] backend (today: [storage.azure]).\n\
             - Run `cargo bench-history collect --local=./bench-history` to record the first benchmark history entry.\n\
             - To seed history for an existing repository, run `cargo bench-history backfill --local=./bench-history <from-commit> <to-commit>` to benchmark a range of past commits.\n\
             - Run `cargo bench-history analyze --local=./bench-history` once you have a few entries to review trends."
        )
    } else {
        format!("Configuration already exists at {display}; leaving it unchanged.")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use cbh_diag::RecordingReporter;
    use futures::executor::block_on;

    use super::*;
    use crate::config_writer::MemoryConfigWriter;
    use crate::wiring::default_config_path;

    /// Tests pass an empty base so `resolve_config_path` leaves the relative
    /// configuration key untouched, keeping the in-memory writer keys relative.
    fn no_workspace() -> &'static Path {
        Path::new("")
    }

    #[test]
    fn fresh_install_writes_the_template_to_the_default_path() {
        let writer = MemoryConfigWriter::default();
        let reporter = RecordingReporter::new();
        let options = InstallOptions::default();

        let outcome = block_on(execute_install(
            &options,
            no_workspace(),
            &writer,
            &reporter,
        ))
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("install should complete: {outcome:?}");
        };
        assert!(
            message.contains("Wrote a starter configuration"),
            "{message}"
        );
        assert!(message.contains("Next steps"), "{message}");
        assert_eq!(
            writer.written(&default_config_path()).as_deref(),
            Some(default_template()),
            "the default template should be written verbatim"
        );
        assert!(
            reporter.contains("wrote the starter template"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn install_honors_an_explicit_config_path() {
        let writer = MemoryConfigWriter::default();
        let reporter = RecordingReporter::new();
        let path = PathBuf::from("custom/bench.toml");
        let options = InstallOptions {
            config_path: Some(path.clone()),
            verbose: false,
        };

        block_on(execute_install(
            &options,
            no_workspace(),
            &writer,
            &reporter,
        ))
        .unwrap();

        assert_eq!(writer.written(&path).as_deref(), Some(default_template()));
    }

    #[test]
    fn install_never_overwrites_an_existing_file() {
        let path = default_config_path();
        let writer = MemoryConfigWriter::with_existing(&path, "# hand-written\n");
        let reporter = RecordingReporter::new();
        let options = InstallOptions::default();

        let outcome = block_on(execute_install(
            &options,
            no_workspace(),
            &writer,
            &reporter,
        ))
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("install should complete: {outcome:?}");
        };
        assert!(message.contains("already exists"), "{message}");
        assert_eq!(
            writer.written(&path).as_deref(),
            Some("# hand-written\n"),
            "the existing file must be left unchanged"
        );
        assert!(
            reporter.contains("left it unchanged"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn install_message_reports_written_and_existing_states() {
        let path = PathBuf::from(".cargo/bench_history.toml");
        let wrote = install_message(&path, true);
        assert!(wrote.contains("Wrote a starter configuration"), "{wrote}");
        assert!(wrote.contains("bench_history.toml"), "{wrote}");
        assert!(wrote.contains("backfill"), "{wrote}");

        let existed = install_message(&path, false);
        assert!(existed.contains("already exists"), "{existed}");
        assert!(existed.contains("bench_history.toml"), "{existed}");
    }

    #[test]
    fn install_message_has_no_incidental_indentation() {
        // The `\n\` string-continuation escape strips the source indentation of
        // each continued line, so no rendered line should start with whitespace.
        let message = install_message(&PathBuf::from(".cargo/bench_history.toml"), true);
        for line in message.lines() {
            assert!(
                !line.starts_with([' ', '\t']),
                "a rendered line is indented: {line:?}"
            );
        }
    }
}
