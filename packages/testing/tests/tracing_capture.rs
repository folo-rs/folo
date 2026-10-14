//! One test owns the process-global subscriber, including all spawned threads.

use std::cell::Cell;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::{Arc, Barrier};
use std::{fmt, thread};

use static_assertions::assert_impl_all;
use testing::{TracingCapture, assert_panics, with_watchdog};
use tracing::{debug, error, field, info, trace, trace_span, warn};

testing::set_allocator!();

assert_impl_all!(TracingCapture: Send, Sync, UnwindSafe, RefUnwindSafe);

/// Keeps capture regressions together because the subscriber owns the process for its lifetime.
///
/// Formatting must run outside the capture lock so a formatter can inspect captured output.
/// See docs/testing.md, "Tracing log coverage", for the isolated-capture boundary.
#[test]
fn captures_every_level_and_formats_fields_across_threads() {
    let capture = TracingCapture::start();
    with_watchdog(move || {
        assert!(capture.snapshot().is_empty());

        trace!("trace canary");
        debug!("debug canary");
        info!("info canary");
        warn!("warn canary");
        error!("error canary");

        let entries = capture.snapshot();
        for level in ["trace", "debug", "info", "warn", "error"] {
            let canary = format!("{level} canary");
            assert!(entries.iter().any(|entry| entry.contains(&canary)));
        }

        let formats = Cell::new(0);
        let value = FormattingProbe {
            formats: &formats,
            capture: &capture,
        };
        trace!(debug = ?value, display = %value, "message {value}");
        assert_eq!(formats.get(), 3);

        let span = trace_span!("span canary", initial = ?value, later = field::Empty);
        assert_eq!(formats.get(), 4);
        span.record("later", field::display(&value));
        assert_eq!(formats.get(), 5);
        span.in_scope(|| trace!("inside span"));
        drop(span);

        assert!(capture.snapshot().iter().any(|entry| {
            entry.contains("span canary")
                && entry.contains("initial=formatted")
                && entry.contains("later=formatted")
        }));

        // Competing writers and multiline fields must still produce whole entries.
        let start = Arc::new(Barrier::new(2));
        let workers = ["first", "second"].map(|marker| {
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                trace!(worker = marker, "worker start\nworker end");
            })
        });
        for worker in workers {
            worker.join().unwrap();
        }
        let entries = capture.snapshot();
        for marker in ["first", "second"] {
            let entry = entries.iter().find(|entry| entry.contains(marker)).unwrap();
            assert!(entry.contains("worker start\nworker end"));
        }
        assert!(entries.iter().all(|entry| !entry.contains('\u{1b}')));

        // A second initialization must not silently capture through the wrong subscriber.
        assert_panics(TracingCapture::start);
        trace!("after rejected initialization");
        assert!(
            capture
                .snapshot()
                .iter()
                .any(|entry| entry.contains("after rejected"))
        );
    });
}

/// Counts lazy formatting and checks that formatting does not hold the capture lock.
struct FormattingProbe<'a> {
    formats: &'a Cell<usize>,
    capture: &'a TracingCapture,
}

impl fmt::Debug for FormattingProbe<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.formats.set(self.formats.get().checked_add(1).unwrap());
        _ = self.capture.snapshot();
        f.write_str("formatted")
    }
}

impl fmt::Display for FormattingProbe<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
