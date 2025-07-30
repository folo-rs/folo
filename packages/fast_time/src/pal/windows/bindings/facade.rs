use std::fmt::Debug;
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;

use crate::pal::windows::BuildTargetBindings;
#[cfg(test)]
use crate::pal::windows::MockBindings;
use crate::pal::windows::bindings::Bindings;

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

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockBindings) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

impl Bindings for BindingsFacade {
    fn get_tick_count_64(&self) -> u64 {
        match self {
            Self::Real(bindings) => bindings.get_tick_count_64(),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.get_tick_count_64(),
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
