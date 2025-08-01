#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
#[cfg(not(debug_assertions))]
use std::marker::PhantomData;

#[cfg(debug_assertions)]
pub(crate) type BacktraceType = Backtrace;
#[cfg(not(debug_assertions))]
pub(crate) type BacktraceType = PhantomData<Backtrace>;

/// Captures a backtrace if both:
///
/// 1. `RUST_BACKTRACE=1` is set.
/// 2. `cfg(debug_assertions)` is enabled (e.g. you are using the default `dev` Cargo profile).
pub(crate) fn capture_backtrace() -> BacktraceType {
    #[cfg(debug_assertions)]
    {
        Backtrace::capture()
    }
    #[cfg(not(debug_assertions))]
    {
        PhantomData
    }
}
