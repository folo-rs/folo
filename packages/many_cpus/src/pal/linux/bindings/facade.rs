use std::fmt::Debug;
use std::io;
#[cfg(test)]
use std::sync::Arc;

use libc::cpu_set_t;

#[cfg(test)]
use crate::pal::linux::MockBindings;
use crate::pal::linux::{Bindings, BuildTargetBindings};

/// Enum to hide the real/mock choice behind a single wrapper type.
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
    fn sched_setaffinity_current(&self, cpuset: &cpu_set_t) -> Result<(), io::Error> {
        match self {
            Self::Real(bindings) => bindings.sched_setaffinity_current(cpuset),
            #[cfg(test)]
            Self::Mock(mock) => mock.sched_setaffinity_current(cpuset),
        }
    }

    fn sched_getcpu(&self) -> i32 {
        match self {
            Self::Real(bindings) => bindings.sched_getcpu(),
            #[cfg(test)]
            Self::Mock(mock) => mock.sched_getcpu(),
        }
    }

    fn sched_getaffinity_current(&self) -> Result<cpu_set_t, io::Error> {
        match self {
            Self::Real(bindings) => bindings.sched_getaffinity_current(),
            #[cfg(test)]
            Self::Mock(mock) => mock.sched_getaffinity_current(),
        }
    }
}

impl Debug for BindingsFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Mock(inner) => inner.fmt(f),
        }
    }
}
