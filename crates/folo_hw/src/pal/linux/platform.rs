use crate::pal::{
    linux::{CurrentBindings, PlatformCore},
    Platform, ProcessorImpl,
};

/// The singleton instance that allows types to work with the current platform.
pub(crate) static CURRENT: CurrentPlatform = CurrentPlatform {
    inner: PlatformCore::new(&CurrentBindings),
};

#[derive(Debug)]
pub(crate) struct CurrentPlatform {
    inner: PlatformCore<CurrentBindings>,
}

impl Platform for CurrentPlatform {
    type Processor = ProcessorImpl;

    fn get_all_processors(&self) -> nonempty::NonEmpty<Self::Processor> {
        self.inner.get_all_processors()
    }

    fn pin_current_thread_to<P>(&self, processors: &nonempty::NonEmpty<P>)
    where
        P: AsRef<Self::Processor>,
    {
        self.inner.pin_current_thread_to(processors);
    }
}
