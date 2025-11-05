use std::fmt::Debug;
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;

#[cfg(test)]
use crate::pal::linux::MockBindings;
use crate::pal::linux::{Bindings, BuildTargetBindings};

#[derive(Clone)]
pub(crate) enum BindingsFacade {
    Real(&'static BuildTargetBindings),

    #[cfg(test)]
    Mock(Arc<MockBindings>),
}

impl BindingsFacade {
    pub(crate) const fn real() -> Self {
        Self::Real(&BuildTargetBindings)
    }
}

impl Bindings for BindingsFacade {
    fn clock_gettime_nanos(&self) -> u128 {
        match self {
            Self::Real(bindings) => bindings.clock_gettime_nanos(),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.clock_gettime_nanos(),
        }
    }

    fn now(&self) -> Instant {
        match self {
            Self::Real(bindings) => bindings.now(),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.now(),
        }
    }
}

impl From<&'static BuildTargetBindings> for BindingsFacade {
    fn from(bindings: &'static BuildTargetBindings) -> Self {
        Self::Real(bindings)
    }
}

#[cfg(test)]
impl From<MockBindings> for BindingsFacade {
    fn from(bindings: MockBindings) -> Self {
        Self::Mock(Arc::new(bindings))
    }
}

impl Debug for BindingsFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(bindings) => bindings.fmt(f),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.fmt(f),
        }
    }
}
