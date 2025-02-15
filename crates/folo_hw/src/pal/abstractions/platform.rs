use std::fmt::Debug;

use nonempty::NonEmpty;

use crate::{pal::ProcessorFacade, ProcessorId};

pub(crate) trait Platform: Debug + Send + Sync + 'static {
    /// Returns all currently available processors.
    ///
    /// The returned collection of processors is sorted by the processor ID for ease of testing.
    /// TODO: Should we just sort in tests instead to avoid baking in some sorting assumptions?
    fn get_all_processors(&self) -> NonEmpty<ProcessorFacade>;

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>;

    /// Gets the ID of the processor currently executing this thread.
    fn current_processor_id(&self) -> ProcessorId;
}
