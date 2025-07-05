//! Platform abstraction trait definitions.

use std::fmt::Debug;
use std::time::Duration;

/// Provides processor time tracking functionality.
///
/// This trait abstracts the underlying platform-specific processor time tracking
/// mechanisms, allowing for both real implementations (using system calls)
/// and fake implementations (for testing).
pub(crate) trait Platform: Debug + Send + Sync + 'static {
    /// Gets the current thread processor time.
    ///
    /// This method returns the current thread processor time as a duration.
    fn thread_time(&self) -> Duration;

    /// Gets the current process processor time.
    ///
    /// This method returns the current process processor time as a duration.
    fn process_time(&self) -> Duration;
}
