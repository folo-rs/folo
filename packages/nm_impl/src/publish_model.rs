use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;
use std::sync::Arc;

#[cfg(test)]
use crate::ObservationBagSnapshot;
use crate::{Magnitude, ObservationBag, ObservationBagSync, Observations};

/// Defines how the metrics related to an event are published for reporting purposes.
///
/// For more information, refer to either the [crate-level documentation][crate] or the
/// documentation of [`Event`][crate::Event].
///
/// This trait is sealed; `nm` provides all implementations.
#[expect(
    private_bounds,
    reason = "The private supertrait seals this public trait."
)]
pub trait PublishModel: PublishModelPrivate {}

/// A publishing model that requires the owner of the event to explicitly publish
/// the event's metrics.
///
/// The owner must call [`MetricsPusher::push()`][1] on the metrics pusher associated with the
/// event at creation time.
///
/// [1]: crate::MetricsPusher::push
#[derive(Debug)]
pub struct Push {
    pub(crate) observations: Rc<ObservationBag>,
}

// Push is single-threaded (!Send, !Sync) and uses interior mutability only for
// metrics tracking. Inconsistent state after a caught panic cannot affect safety.
impl UnwindSafe for Push {}
impl RefUnwindSafe for Push {}

impl PublishModel for Push {}
impl PublishModelPrivate for Push {
    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn snapshot(&self) -> ObservationBagSnapshot {
        self.observations.snapshot()
    }

    // Callgrind and disassembly showed that the default cross-CGU decision left the complete
    // insertion chain out of line. Inlining its forwarders removes a call from every observation.
    #[inline]
    fn insert(&self, magnitude: Magnitude, count: usize) {
        self.observations.insert(magnitude, count);
    }
}

/// A publishing model that requires no action from the owner of an event.
///
/// Data is automatically pulled from the event when a report is generated.
#[derive(Debug)]
pub struct Pull {
    // Reports retain and read this observation state from other threads even though each
    // Event handle and its registration belong to one thread.
    pub(crate) observations: Arc<ObservationBagSync>,
}

impl PublishModel for Pull {}
impl PublishModelPrivate for Pull {
    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn snapshot(&self) -> ObservationBagSnapshot {
        self.observations.snapshot()
    }

    // Callgrind and disassembly showed that the default cross-CGU decision left the complete
    // insertion chain out of line. Inlining its forwarders removes a call from every observation.
    #[inline]
    fn insert(&self, magnitude: Magnitude, count: usize) {
        self.observations.insert(magnitude, count);
    }
}

/// Defines the operations required from a publishing model.
///
/// As a private supertrait, this also limits [`PublishModel`] implementations to this crate.
pub(crate) trait PublishModelPrivate {
    /// Records `count` observations of the given `magnitude` for the event.
    fn insert(&self, magnitude: Magnitude, count: usize);

    /// Takes a snapshot of the recorded observations for this event.
    #[cfg(test)]
    fn snapshot(&self) -> ObservationBagSnapshot;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Push: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Pull: UnwindSafe, RefUnwindSafe);
}
