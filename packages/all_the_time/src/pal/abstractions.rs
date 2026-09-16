//! Platform abstraction trait definitions.

use std::fmt::Debug;
use std::time::Duration;

use crate::Report;

/// Provides processor clocks and external report output.
///
/// Session lifecycle decisions stay above this boundary so fake clocks and
/// captured reports can exercise them without operating-system interactions.
pub(crate) trait Platform: Debug + Send + Sync + 'static {
    /// Gets the current thread processor time.
    ///
    /// This method returns the current thread processor time as a duration.
    fn thread_time(&self) -> Duration;

    /// Gets the current process processor time.
    ///
    /// This method returns the current process processor time as a duration.
    fn process_time(&self) -> Duration;

    /// Prints a report to stdout.
    fn print_to_stdout(&self, report: &Report);

    /// Writes a report to the Cargo target directory.
    fn write_to_target(&self, report: &Report);
}
