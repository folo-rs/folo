//! The `install` command: write a starter `.cargo/bench_history.toml` if one is
//! absent, then point the user at it. An existing file is never overwritten.

use std::path::Path;

use crate::config::default_template;
use crate::config_writer::{ConfigWriter, TokioConfigWriter};
use crate::wiring::default_config_path;
use crate::{InstallOptions, RunError, RunOutcome};

/// Executes the `install` command against the real filesystem.
///
/// # Errors
///
/// Returns [`RunError::Io`] if the configuration file cannot be written.
pub(crate) async fn execute(options: &InstallOptions) -> Result<RunOutcome, RunError> {
    let writer = TokioConfigWriter;
    execute_install(options, &writer).await
}

/// The filesystem-agnostic orchestration, generic over the [`ConfigWriter`] port
/// so tests can drive it with an in-memory fake.
async fn execute_install<W: ConfigWriter>(
    options: &InstallOptions,
    writer: &W,
) -> Result<RunOutcome, RunError> {
    let path = options
        .config_path
        .clone()
        .unwrap_or_else(default_config_path);

    let written = writer.write_new(&path, default_template()).await?;

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
             - Edit it to set the [storage] backend for this workspace.\n\
             - Run `cargo bench-history run` to record the first benchmark history entry.\n\
             - To seed history for an existing repository, run `cargo bench-history backfill --from <commit> --to <commit>` to benchmark a range of past commits.\n\
             - Run `cargo bench-history analyze` once you have a few entries to review trends."
        )
    } else {
        format!("Configuration already exists at {display}; leaving it unchanged.")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use futures::executor::block_on;

    use crate::config_writer::MemoryConfigWriter;

    use super::*;

    #[test]
    fn fresh_install_writes_the_template_to_the_default_path() {
        let writer = MemoryConfigWriter::default();
        let options = InstallOptions::default();

        let outcome = block_on(execute_install(&options, &writer)).expect("install should succeed");

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
    }

    #[test]
    fn install_honors_an_explicit_config_path() {
        let writer = MemoryConfigWriter::default();
        let path = PathBuf::from("custom/bench.toml");
        let options = InstallOptions {
            config_path: Some(path.clone()),
        };

        block_on(execute_install(&options, &writer)).expect("install should succeed");

        assert_eq!(writer.written(&path).as_deref(), Some(default_template()));
    }

    #[test]
    fn install_never_overwrites_an_existing_file() {
        let path = default_config_path();
        let writer = MemoryConfigWriter::with_existing(&path, "# hand-written\n");
        let options = InstallOptions::default();

        let outcome = block_on(execute_install(&options, &writer)).expect("install should succeed");

        let RunOutcome::Completed { message } = outcome else {
            panic!("install should complete: {outcome:?}");
        };
        assert!(message.contains("already exists"), "{message}");
        assert_eq!(
            writer.written(&path).as_deref(),
            Some("# hand-written\n"),
            "the existing file must be left unchanged"
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
