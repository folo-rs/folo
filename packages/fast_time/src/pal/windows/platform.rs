use crate::pal::windows::BindingsFacade;
use crate::pal::{Platform, TimeSourceImpl};

/// Singleton instance of `BuildTargetPlatform`, used by public API types
/// to hook up to the correct PAL implementation.
pub(crate) static BUILD_TARGET_PLATFORM: BuildTargetPlatform =
    BuildTargetPlatform::new(BindingsFacade::real());

#[derive(Debug)]
pub(crate) struct BuildTargetPlatform {
    bindings: BindingsFacade,
}

impl BuildTargetPlatform {
    // Only executed in const context.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(crate) const fn new(bindings: BindingsFacade) -> Self {
        Self { bindings }
    }
}

impl Platform for BuildTargetPlatform {
    type TimeSource = TimeSourceImpl;

    fn new_time_source(&self) -> Self::TimeSource {
        Self::TimeSource::new(self.bindings.clone())
    }
}
