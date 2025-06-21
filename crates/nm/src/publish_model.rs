use std::{rc::Rc, sync::Arc};

#[cfg(test)]
use crate::ObservationBagSnapshot;
use crate::{Magnitude, ObservationBag, ObservationBagSync, Observations, Sealed};

/// Defines how the metrics related to an event are published for reporting purposes.
///
/// For more information, refer to either the [crate-level documentation][crate] or the
/// documentation of [`Event`][crate::Event].
///
/// This trait is sealed and can only be implemented in the `nm` crate.
#[expect(private_bounds, reason = "intentionally sealed trait")]
pub trait PublishModel: PublishModelPrivate + Sealed {}

/// Privately accessible functionality expected from implementations of `PublishModel`.
pub(crate) trait PublishModelPrivate {
    /// Records `count` observations of the given `magnitude` for the event.
    fn insert(&self, magnitude: Magnitude, count: usize);

    /// Takes a snapshot of the recorded observations for this event.
    #[cfg(test)]
    fn snapshot(&self) -> ObservationBagSnapshot;
}

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

impl Sealed for Push {}
impl PublishModel for Push {}
impl PublishModelPrivate for Push {
    #[cfg(test)]
    fn snapshot(&self) -> ObservationBagSnapshot {
        self.observations.snapshot()
    }

    fn insert(&self, magnitude: Magnitude, count: usize) {
        self.observations.insert(magnitude, count);
    }
}

/// A publishing model that requires no action from the owner of an event.
///
/// Data is automatically pulled from the event when a report is generated.
#[derive(Debug)]
pub struct Pull {
    // While an event is a single-threaded type, we need to use an Arc here because the
    // observations may still be read (without locking) from other threads.
    pub(crate) observations: Arc<ObservationBagSync>,
}

impl Sealed for Pull {}
impl PublishModel for Pull {}
impl PublishModelPrivate for Pull {
    #[cfg(test)]
    fn snapshot(&self) -> ObservationBagSnapshot {
        self.observations.snapshot()
    }

    fn insert(&self, magnitude: Magnitude, count: usize) {
        self.observations.insert(magnitude, count);
    }
}
