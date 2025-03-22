use std::{fmt::Debug, io};

#[cfg(test)]
use std::sync::Arc;

use libc::cpu_set_t;

use crate::pal::linux::{Bindings, BuildTargetBindings};

#[cfg(test)]
use crate::pal::linux::MockBindings;

/// Enum to hide the real/mock choice behind a single wrapper type.
#[derive(Clone)]
pub(crate) enum BindingsFacade {
    Real(&'static BuildTargetBindings),

    #[cfg(test)]
    Mock(Arc<MockBindings>),
}

impl BindingsFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    pub const fn real() -> Self {
        BindingsFacade::Real(&BuildTargetBindings)
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    #[cfg(test)]
    pub fn from_mock(mock: MockBindings) -> Self {
        BindingsFacade::Mock(Arc::new(mock))
    }
}

impl Bindings for BindingsFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn sched_setaffinity_current(&self, cpuset: &cpu_set_t) -> Result<(), io::Error> {
        match self {
            BindingsFacade::Real(bindings) => bindings.sched_setaffinity_current(cpuset),
            #[cfg(test)]
            BindingsFacade::Mock(mock) => mock.sched_setaffinity_current(cpuset),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn sched_getcpu(&self) -> i32 {
        match self {
            BindingsFacade::Real(bindings) => bindings.sched_getcpu(),
            #[cfg(test)]
            BindingsFacade::Mock(mock) => mock.sched_getcpu(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn sched_getaffinity_current(&self) -> Result<cpu_set_t, io::Error> {
        match self {
            BindingsFacade::Real(bindings) => bindings.sched_getaffinity_current(),
            #[cfg(test)]
            BindingsFacade::Mock(mock) => mock.sched_getaffinity_current(),
        }
    }
}

impl Debug for BindingsFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Mock(inner) => inner.fmt(f),
        }
    }
}
