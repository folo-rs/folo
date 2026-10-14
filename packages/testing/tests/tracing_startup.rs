//! Ordinary parallel tests share permanent logging without sharing captured output.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fmt, thread};

use testing::{initialize_tracing, with_watchdog};
use tracing::{debug, error, info, trace, warn};

testing::set_allocator!();
// Repeated module-scope invocations must neither collide nor install competing subscribers.
testing::init_tracing!();
testing::init_tracing!();

/// Executable initialization must enable real formatting before any test performs its own setup.
#[test]
fn logging_is_enabled_before_test_setup() {
    assert!(tracing::enabled!(tracing::Level::TRACE));
    let formats = AtomicUsize::new(0);
    let value = FormatProbe(&formats);
    trace!(?value);
    debug!(?value);
    info!(?value);
    warn!(?value);
    error!(?value);
    assert_eq!(formats.load(Ordering::Relaxed), 5);
}

/// Worker threads must inherit permanent logging without capture scopes or per-thread setup.
#[test]
fn background_threads_format_without_local_subscribers() {
    // This test must also pass when selected alone by nextest or libtest.
    assert!(tracing::enabled!(tracing::Level::TRACE));
    with_watchdog(|| {
        let formats = AtomicUsize::new(0);
        thread::scope(|scope| {
            // Competing callers must not replace the constructor's subscriber.
            for _ in 0..2 {
                scope.spawn(|| {
                    initialize_tracing();
                    trace!(value = ?FormatProbe(&formats));
                });
            }
        });
        assert_eq!(formats.load(Ordering::Relaxed), 2);
    });
}

/// Makes real formatting observable even though the subscriber discards its output.
struct FormatProbe<'a>(&'a AtomicUsize);

impl fmt::Debug for FormatProbe<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fetch_add(1, Ordering::Relaxed);
        f.write_str("formatted")
    }
}
