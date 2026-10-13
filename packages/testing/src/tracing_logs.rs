// Capture writer adapted from microsoft/oxidizer's testing_aids tracing_logs helpers.
// Copyright (c) Microsoft Corporation. Licensed under the MIT License.
// See ../LICENSE-oxidizer and docs/testing.md, "Tracing log coverage", at the workspace root.

use std::io::{self, Write};
use std::mem;
use std::sync::{Arc, Mutex, Once};

use tracing::level_filters::{LevelFilter, STATIC_MAX_LEVEL};
use tracing::subscriber::set_global_default;
use tracing_subscriber::fmt::MakeWriter;

/// Captures fully formatted tracing output from every thread in a test process.
///
/// Use a dedicated integration-test binary containing a single test. Start capture
/// before constructing fixtures or starting workers, and do not install other
/// subscribers, including [`init_tracing!`][crate::init_tracing] or thread-local
/// subscribers, in that process. Ordinary tests use [`init_tracing!`][crate::init_tracing]
/// instead when they only need logging expressions and formatting to execute.
///
/// Every level is enabled, independently of environment filters. Events and span
/// fields are formatted synchronously. Output has no timestamps or terminal colors.
/// Dropping this handle does not uninstall the subscriber or disable formatting.
#[derive(Debug)]
pub struct TracingCapture {
    entries: Arc<Mutex<Vec<String>>>,
}

impl TracingCapture {
    /// Enables tracing for the process and starts capturing formatted output.
    ///
    /// # Panics
    ///
    /// Panics if a global subscriber is already installed or compile-time filtering
    /// disables any tracing level.
    #[must_use]
    pub fn start() -> Self {
        let entries = Arc::new(Mutex::new(Vec::new()));
        install_tracing(CaptureWriterFactory {
            entries: Arc::clone(&entries),
        });

        Self { entries }
    }

    /// Returns the formatted entries captured so far without ending capture.
    ///
    /// Join background workers before taking a final snapshot. Synchronous formatting
    /// does not imply that those workers have finished emitting events.
    #[must_use]
    pub fn snapshot(&self) -> Vec<String> {
        self.entries.lock().unwrap().clone()
    }
}

/// Enables silent tracing with full field and message formatting for the process.
///
/// Repeated calls share one subscriber, including concurrent calls. Prefer
/// [`init_tracing!`][crate::init_tracing] in test executables so initialization runs
/// before tests construct fixtures or start workers. This function also supports
/// explicit initialization by a custom harness.
///
/// Output is discarded after formatting, not before it. Logging remains enabled
/// for every level and thread for the process lifetime, including mutation testing.
///
/// # Panics
///
/// Panics if another global subscriber is installed, compile-time filtering disables
/// a tracing level, or an earlier initialization panicked.
pub fn initialize_tracing() {
    static INITIALIZER: Once = Once::new();

    INITIALIZER.call_once(|| install_tracing(io::sink));
}

/// Keeps the coverage behavior identical whether formatted output is captured or discarded.
fn install_tracing<W>(writer: W)
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    assert_eq!(STATIC_MAX_LEVEL, LevelFilter::TRACE);
    // A live formatting layer is essential: an interested no-op subscriber
    // evaluates expressions but never calls their Debug/Display implementations.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(LevelFilter::TRACE)
        .without_time()
        .with_ansi(false)
        .with_writer(writer)
        .finish();
    set_global_default(subscriber).unwrap();
}

/// Keeps each event's output together even when threads emit concurrently.
struct CaptureWriterFactory {
    entries: Arc<Mutex<Vec<String>>>,
}

impl<'a> MakeWriter<'a> for CaptureWriterFactory {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter {
            entries: Arc::clone(&self.entries),
            pending: Vec::new(),
        }
    }
}

/// Buffers one formatter invocation without holding the shared capture lock.
struct CaptureWriter {
    entries: Arc<Mutex<Vec<String>>>,
    pending: Vec<u8>,
}

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        if self.pending.is_empty() {
            return;
        }

        // Conversion and user formatting happen outside the lock, so a formatting
        // panic cannot poison capture or deadlock a formatter that inspects it.
        let entry = String::from_utf8(mem::take(&mut self.pending)).unwrap();
        self.entries.lock().unwrap().push(entry);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fmt;

    use tracing::{field, trace, trace_span};

    use super::*;
    use crate::assert_panics_with;

    #[test]
    fn startup_formats_private_unit_test_fields() {
        let expressions = Cell::new(0);
        let formats = Cell::new(0);
        let value = FormatProbe(&formats);
        // No initializer call here: the crate-root constructor must precede the test.
        trace!(
            expression = {
                expressions.set(1);
                "evaluated"
            },
            debug = ?value,
            display = %value,
            "message {value}"
        );
        assert_eq!(expressions.get(), 1);
        assert_eq!(formats.get(), 3);

        let span = trace_span!("private fields", initial = ?value, later = field::Empty);
        span.record("later", field::display(&value));
        assert_eq!(formats.get(), 5);
    }

    #[test]
    fn initialization_is_idempotent_without_disabling_formatting() {
        initialize_tracing();
        initialize_tracing();
        let formats = Cell::new(0);
        trace!(value = ?FormatProbe(&formats));
        assert_eq!(formats.get(), 1);
    }

    #[test]
    fn discarded_output_still_propagates_formatting_panics() {
        assert_panics_with(
            || trace!(value = ?PanickingFormat),
            |message| assert!(message.contains("format canary")),
        );
        let formats = Cell::new(0);
        trace!(value = ?FormatProbe(&formats));
        assert_eq!(formats.get(), 1);
    }

    /// Private data can reach logging without exposing a test-only public API.
    struct FormatProbe<'a>(&'a Cell<usize>);

    impl fmt::Debug for FormatProbe<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.set(self.0.get().checked_add(1).unwrap());
            f.write_str("formatted")
        }
    }

    impl fmt::Display for FormatProbe<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            fmt::Debug::fmt(self, f)
        }
    }

    /// Detects an initializer that enables events but skips field formatting.
    struct PanickingFormat;

    impl fmt::Debug for PanickingFormat {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("format canary");
        }
    }
}
