use crate::{pal::PlatformFacade, Processor};

/// Tracks/identifies the current processor and related metadata.
/// 
/// The meaning of "current" is deceptively simple: the current processor is whatever processor
/// is executing the code at the moment the "get current" logic is called.
/// 
/// Now, most threads can move between processors, so the current processor can change over time.
/// This means that there is a certain "time of check to time of use" discrepancy that can occur.
/// This is unavoidable if your threads are floating - just live with it.
/// 
/// To avoid this problem, you have to use pinned threads, assigned to execute on either a specific
/// processor or set of processors that the desired quality you care about (e.g. memory region).
pub(crate) struct CurrentTracker {
    pal: PlatformFacade,
}

impl CurrentTracker {
    pub(crate) fn new(pal: PlatformFacade) -> Self {
        Self { pal }
    }

    pub(crate) fn current_processor(&self) -> &Processor {
        todo!()
    }
}