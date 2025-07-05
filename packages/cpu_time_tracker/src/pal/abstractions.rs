//! Platform abstraction trait definitions.

use std::fmt::Debug;
use std::time::Duration;

/// Provides CPU time tracking functionality.
///
/// This trait abstracts the underlying platform-specific CPU time tracking
/// mechanisms, allowing for both real implementations (using system calls)
/// and fake implementations (for testing).
pub(crate) trait Platform: Debug + Send + Sync + 'static {
    /// Gets the current thread CPU time.
    ///
    /// This method returns the current thread CPU time as a duration.
    fn thread_time(&self) -> Duration;

    /// Gets the current process CPU time.
    ///
    /// This method returns the current process CPU time as a duration.
    fn process_time(&self) -> Duration;
}
