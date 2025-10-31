use std::fmt::Debug;
use std::time::Instant;

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Bindings: Debug + Send + Sync + 'static {
    fn clock_gettime_nanos(&self) -> u128;

    // We also put this here because Rust does not (yet) support a proper clock abstraction,
    // so without this we have nothing to mock. This just provides a mock wrapper around `Instant`.
    fn now(&self) -> Instant;
}
