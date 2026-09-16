//! Writing to the user's streams without panicking.
//!
//! The `print!` family panics when the stream behind it fails, which turns a
//! consumer that closed a pipe into an unwind through a command boundary. The
//! two kinds of output `dure` produces want different answers to that, so both
//! are stated here:
//!
//! * **Result output** — the session table, the resume prompt — is what the
//!   command was asked for, so a stream that cannot take it fails the command.
//! * **Diagnostics** — verbose notes, warnings, the session banner, the final
//!   error message — explain what happened. A stream that cannot take one is
//!   not worth failing over, least of all after a session has been committed,
//!   so they are best effort.

use std::fmt::Arguments;
use std::io::{self, Write};

/// Writes result output the command's success depends on.
///
/// # Errors
///
/// Returns the stream failure so the caller can decide what its command should
/// report.
// Selecting the real stdout requires process-level observation (tests/cli.rs).
// The required write/flush policy is unit-tested in write_result.
// Ref: docs/testing.md, "Mutation testing coverage and skipping mutations".
#[cfg_attr(test, mutants::skip)]
pub(crate) fn print_line(message: Arguments<'_>) -> io::Result<()> {
    write_result(&mut io::stdout().lock(), format_args!("{message}\n"))
}

/// Writes a diagnostic, giving up quietly if the stream will not take it.
// Selecting the real stderr requires process-level observation (tests/cli.rs).
// Diagnostic emission and failure handling are unit-tested in write_diagnostic.
// Ref: docs/testing.md, "Mutation testing coverage and skipping mutations".
#[cfg_attr(test, mutants::skip)]
pub(crate) fn note_line(message: Arguments<'_>) {
    write_diagnostic(&mut io::stderr().lock(), message);
}

/// Writes a prompt, without the newline the answer will follow.
///
/// # Errors
///
/// Returns the stream failure: a prompt nobody can see is not worth blocking a
/// read on.
// Selecting the real stderr requires process-level observation. Prompt decisions
// are covered by the command tests; write_result covers required write/flush behavior.
// Ref: docs/testing.md, "Mutation testing coverage and skipping mutations".
#[cfg_attr(test, mutants::skip)]
pub(crate) fn print_prompt(message: Arguments<'_>) -> io::Result<()> {
    write_result(&mut io::stderr().lock(), message)
}

fn write_result(writer: &mut impl Write, message: Arguments<'_>) -> io::Result<()> {
    write!(writer, "{message}")?;
    writer.flush()
}

fn write_diagnostic(writer: &mut impl Write, message: Arguments<'_>) {
    _ = writeln!(writer, "{message}");
    // Flush even after a failed write: earlier buffered diagnostics may still be deliverable.
    _ = writer.flush();
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::ErrorKind;

    use super::*;

    /// Records bytes visible at flush and injects stream failures without real I/O.
    #[derive(Default)]
    struct RecordingWriter {
        bytes: Vec<u8>,
        flushed: Option<Vec<u8>>,
        write_error: Option<ErrorKind>,
        flush_error: Option<ErrorKind>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(kind) = self.write_error {
                return Err(kind.into());
            }
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushed = Some(self.bytes.clone());
            self.flush_error.map_or(Ok(()), |kind| Err(kind.into()))
        }
    }

    #[test]
    fn result_formats_and_flushes_without_adding_a_newline() {
        let mut writer = RecordingWriter::default();
        let value = "payload";

        write_result(&mut writer, format_args!("result {value}")).unwrap();

        assert_eq!(writer.bytes, b"result payload");
        assert_eq!(
            writer.flushed.as_deref(),
            Some(b"result payload".as_slice())
        );
    }

    #[test]
    fn result_propagates_write_failure_without_flushing() {
        let mut writer = RecordingWriter {
            write_error: Some(ErrorKind::BrokenPipe),
            flush_error: Some(ErrorKind::PermissionDenied),
            ..RecordingWriter::default()
        };

        let error = write_result(&mut writer, format_args!("result")).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        assert!(writer.bytes.is_empty());
        assert!(writer.flushed.is_none());
    }

    #[test]
    fn result_propagates_flush_failure() {
        let mut writer = RecordingWriter {
            flush_error: Some(ErrorKind::BrokenPipe),
            ..RecordingWriter::default()
        };

        let error = write_result(&mut writer, format_args!("result")).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        assert_eq!(writer.flushed.as_deref(), Some(b"result".as_slice()));
    }

    #[test]
    fn diagnostic_formats_a_line_and_flushes() {
        let mut writer = RecordingWriter::default();
        let value = "payload";

        write_diagnostic(&mut writer, format_args!("note {value}"));

        assert_eq!(writer.bytes, b"note payload\n");
        assert_eq!(
            writer.flushed.as_deref(),
            Some(b"note payload\n".as_slice())
        );
    }

    #[test]
    fn diagnostic_still_flushes_after_write_failure() {
        let mut writer = RecordingWriter {
            bytes: b"earlier note".to_vec(),
            write_error: Some(ErrorKind::BrokenPipe),
            ..RecordingWriter::default()
        };

        write_diagnostic(&mut writer, format_args!("next note"));

        assert_eq!(writer.flushed.as_deref(), Some(b"earlier note".as_slice()));
    }

    #[test]
    fn diagnostic_tolerates_flush_failure() {
        let mut writer = RecordingWriter {
            flush_error: Some(ErrorKind::BrokenPipe),
            ..RecordingWriter::default()
        };

        write_diagnostic(&mut writer, format_args!("note"));

        assert_eq!(writer.flushed.as_deref(), Some(b"note\n".as_slice()));
    }

    #[test]
    fn diagnostic_tolerates_both_failures() {
        let mut writer = RecordingWriter {
            write_error: Some(ErrorKind::BrokenPipe),
            flush_error: Some(ErrorKind::PermissionDenied),
            ..RecordingWriter::default()
        };

        write_diagnostic(&mut writer, format_args!("note"));

        assert_eq!(writer.flushed.as_deref(), Some(b"".as_slice()));
    }
}
