use crate::pal::{
    windows::{BindingsImpl, PlatformCore},
    Platform, ProcessorImpl,
};

/// The singleton instance that allows types to work with the current platform.
pub(crate) static CURRENT: PlatformImpl = PlatformImpl {
    inner: PlatformCore::new(&BindingsImpl),
};

#[derive(Copy, Clone, Debug, Eq, Ord, Hash, PartialEq, PartialOrd)]
pub(crate) struct PlatformImpl {
    inner: PlatformCore<BindingsImpl>,
}

impl Platform for PlatformImpl {
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
