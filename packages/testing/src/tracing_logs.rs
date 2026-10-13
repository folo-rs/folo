// Capture writer adapted from microsoft/oxidizer's testing_aids tracing_logs helpers.
// Copyright (c) Microsoft Corporation. Licensed under the MIT License.
// See ../LICENSE-oxidizer and docs/testing.md, "Tracing log coverage", at the workspace root.

use std::io::{self, Write};
use std::mem;
use std::sync::{Arc, Mutex};

use tracing::level_filters::{LevelFilter, STATIC_MAX_LEVEL};
use tracing::subscriber::set_global_default;
use tracing_subscriber::fmt::MakeWriter;

/// Captures fully formatted tracing output from every thread in a test process.
///
/// Use a dedicated integration-test binary containing a single test. Start capture
/// before constructing fixtures or starting workers, and do not install other
/// subscribers, including thread-local subscribers, in that process.
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
        assert_eq!(STATIC_MAX_LEVEL, LevelFilter::TRACE);

        let entries = Arc::new(Mutex::new(Vec::new()));
        // A live formatting layer is essential: an interested no-op subscriber
        // evaluates expressions but never calls their Debug/Display implementations.
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(LevelFilter::TRACE)
            .without_time()
            .with_ansi(false)
            .with_writer(CaptureWriterFactory {
                entries: Arc::clone(&entries),
            })
            .finish();
        set_global_default(subscriber).unwrap();

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
