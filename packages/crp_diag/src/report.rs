use std::fmt;
use std::io::{self, Write};
use std::panic::RefUnwindSafe;

/// Receives diagnostic bytes without choosing a process stream.
pub trait DiagnosticSink: fmt::Debug + Send + Sync + RefUnwindSafe {
    fn write(&self, text: &str) -> io::Result<()>;
}

/// The process stderr destination selected by the application shell.
#[derive(Debug)]
pub struct Stderr;

impl DiagnosticSink for Stderr {
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
    pub enabled: bool,
    sink: &'a dyn DiagnosticSink,
}

impl<'a> Verbose<'a> {
    #[must_use]
    pub fn new(enabled: bool, sink: &'a dyn DiagnosticSink) -> Self {
        Self { enabled, sink }
    }

    pub fn sink(self) -> &'a dyn DiagnosticSink {
        self.sink
    }

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

/// Receives lazy notes so decision tests need no process-global stream capture.
pub trait NoteSink {
    fn note(&self, message: impl FnOnce() -> String);
}

impl NoteSink for Verbose<'_> {
    fn note(&self, message: impl FnOnce() -> String) {
        (*self).note(message);
    }
}

#[cfg(any(test, feature = "private-test-util"))]
#[cfg_attr(coverage_nightly, coverage(off))]
impl NoteSink for std::cell::RefCell<Vec<String>> {
    fn note(&self, message: impl FnOnce() -> String) {
        self.borrow_mut().push(message());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Mutex;

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Verbose<'static>: Send, Sync, std::panic::UnwindSafe);

    /// Captures the bytes given to a destination without opening a process stream.
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
    fn recording_and_discard_preserve_lazy_note_behavior() {
        let recording = std::cell::RefCell::new(Vec::new());
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
