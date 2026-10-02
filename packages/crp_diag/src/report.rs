use std::cell::RefCell;
use std::fmt;
use std::io::{self, Write};
use std::panic::RefUnwindSafe;

/// Receives diagnostic text without choosing a process stream.
///
/// Sinks are shared with process-output reader threads and may be borrowed across cleanup-time
/// unwind handling. Interior state must be synchronized and preserve ref-unwind-safe invariants.
pub trait DiagnosticSink: fmt::Debug + Send + Sync + RefUnwindSafe {
    fn write(&self, text: &str) -> io::Result<()>;
}

/// Receives lazy notes so decision tests need no process-global stream capture.
pub trait NoteSink {
    fn note(&self, message: impl FnOnce() -> String);
}

/// Writes an unconditional diagnostic using ordinary process-output failure behavior.
///
/// # Panics
///
/// Panics if the selected destination rejects the write.
pub fn diagnostic(sink: &dyn DiagnosticSink, text: &str) {
    if let Err(error) = sink.write(text) {
        panic!("failed to write diagnostic output: {error}");
    }
}

/// The process stderr destination selected by the application shell.
#[derive(Debug)]
pub struct Stderr;

impl DiagnosticSink for Stderr {
    // Process-stream delivery is covered by the executable's stderr integration assertions.
    #[cfg_attr(test, mutants::skip)]
    fn write(&self, text: &str) -> io::Result<()> {
        io::stderr().lock().write_all(text.as_bytes())
    }
}

/// Discards output for operations whose caller does not request diagnostics.
#[derive(Debug)]
pub struct Discard;

impl DiagnosticSink for Discard {
    fn write(&self, _text: &str) -> io::Result<()> {
        Ok(())
    }
}

/// Enables explanatory notes on a caller-selected diagnostic destination.
#[derive(Clone, Copy, Debug)]
pub struct Verbose<'a> {
    enabled: bool,
    sink: &'a dyn DiagnosticSink,
}

impl<'a> Verbose<'a> {
    #[must_use]
    pub fn new(enabled: bool, sink: &'a dyn DiagnosticSink) -> Self {
        Self { enabled, sink }
    }

    #[must_use]
    pub fn sink(self) -> &'a dyn DiagnosticSink {
        self.sink
    }

    #[must_use]
    pub fn enabled(self) -> bool {
        self.enabled
    }

    /// Builds a note only when enabled, avoiding formatting work on the nonverbose path.
    ///
    /// The tool prefix attributes notes among interleaved subprocess output.
    pub fn note(self, message: impl FnOnce() -> String) {
        if self.enabled {
            // Notes are advisory: a closed destination must not abort otherwise successful work.
            // Assemble one line before writing so concurrent notes cannot interleave mid-line.
            // Ref: docs/implementation.md.
            let line = format!("[release-plan] {}\n", message());
            drop(self.sink.write(&line));
        }
    }
}

impl NoteSink for Verbose<'_> {
    fn note(&self, message: impl FnOnce() -> String) {
        (*self).note(message);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl NoteSink for RefCell<Vec<String>> {
    fn note(&self, message: impl FnOnce() -> String) {
        self.borrow_mut().push(message());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::UnwindSafe;
    use std::sync::Mutex;

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Verbose<'static>: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Stderr: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Discard: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(&'static dyn DiagnosticSink: Send, Sync, UnwindSafe, RefUnwindSafe);

    /// Captures diagnostic text without opening a process stream.
    #[derive(Debug, Default)]
    struct Recording(Mutex<Vec<String>>);

    impl DiagnosticSink for Recording {
        fn write(&self, text: &str) -> io::Result<()> {
            self.0.lock().unwrap().push(text.to_owned());
            Ok(())
        }
    }

    /// Models a closed pipe without consulting the operating system.
    #[derive(Debug)]
    struct Closed;

    impl DiagnosticSink for Closed {
        fn write(&self, _text: &str) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn disabled_notes_do_not_build_messages() {
        let recording = Recording::default();
        NoteSink::note(&Verbose::new(false, &recording), || {
            panic!("disabled notes must not evaluate their closure");
        });
        assert!(recording.0.lock().unwrap().is_empty());
    }

    #[test]
    fn verbose_accessors_preserve_policy_and_selected_destination() {
        let first = Recording::default();
        let second = Recording::default();
        let enabled = Verbose::new(true, &first);
        let disabled = Verbose::new(false, &second);
        assert!(enabled.enabled());
        assert!(!disabled.enabled());
        enabled.sink().write("first canary").unwrap();
        disabled.sink().write("second canary").unwrap();
        assert_eq!(*first.0.lock().unwrap(), ["first canary"]);
        assert_eq!(*second.0.lock().unwrap(), ["second canary"]);
    }

    #[test]
    fn enabled_notes_keep_the_prefix_and_line_boundary() {
        let recording = Recording::default();
        NoteSink::note(&Verbose::new(true, &recording), || "decision".to_owned());
        assert_eq!(*recording.0.lock().unwrap(), ["[release-plan] decision\n"]);
    }

    #[test]
    fn advisory_notes_tolerate_a_closed_destination() {
        let mut built = false;
        Verbose::new(true, &Closed).note(|| {
            built = true;
            "decision".to_owned()
        });
        assert!(built);
    }

    #[test]
    fn unconditional_diagnostics_retain_write_failure_behavior() {
        ::testing::assert_panics(|| diagnostic(&Closed, "diagnostic"));
        let recording = Recording::default();
        diagnostic(&recording, "raw output\n");
        assert_eq!(*recording.0.lock().unwrap(), ["raw output\n"]);
    }

    #[test]
    fn recording_and_discard_preserve_lazy_note_behavior() {
        let recording = RefCell::new(Vec::new());
        recording.note(|| "recorded".to_owned());
        assert_eq!(*recording.borrow(), ["recorded"]);
        let mut built = false;
        Verbose::new(true, &Discard).note(|| {
            built = true;
            "discarded".to_owned()
        });
        assert!(built);
    }
}
