use crate::pal::{
    windows::{CurrentBindings, PlatformCore},
    Platform, ProcessorImpl,
};

/// The singleton instance that allows types to work with the current platform.
pub(crate) static CURRENT: CurrentPlatform = CurrentPlatform {
    core: PlatformCore::new(&CurrentBindings),
};

#[derive(Debug)]
pub(crate) struct CurrentPlatform {
    core: PlatformCore<CurrentBindings>,
}

impl Platform for CurrentPlatform {
    type Processor = ProcessorImpl;

    fn get_all_processors(&self) -> nonempty::NonEmpty<Self::Processor> {
        self.core.get_all_processors()
    }

    fn pin_current_thread_to<P>(&self, processors: &nonempty::NonEmpty<P>)
    where
        P: AsRef<Self::Processor>,
    {
        self.core.pin_current_thread_to(processors);
    }
}
