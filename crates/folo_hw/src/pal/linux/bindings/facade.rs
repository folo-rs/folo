use std::{fmt::Debug, io};

#[cfg(test)]
use std::sync::Arc;

use libc::cpu_set_t;

use crate::pal::linux::{Bindings, BuildTargetBindings};

#[cfg(test)]
use crate::pal::linux::MockBindings;

/// Enum to hide the real/mock choice behind a single wrapper type.
#[derive(Clone, Debug)]
pub(crate) enum BindingsFacade {
    Real(&'static BuildTargetBindings),

    #[cfg(test)]
    Mock(Arc<MockBindings>),
}

impl BindingsFacade {
    pub const fn real() -> Self {
        BindingsFacade::Real(&BuildTargetBindings)
    }

    #[cfg(test)]
    pub fn from_mock(mock: MockBindings) -> Self {
        BindingsFacade::Mock(Arc::new(mock))
    }
}

impl Bindings for BindingsFacade {
    fn sched_setaffinity_current(&self, cpuset: &cpu_set_t) -> Result<(), io::Error> {
        match self {
            BindingsFacade::Real(bindings) => bindings.sched_setaffinity_current(cpuset),
            #[cfg(test)]
            BindingsFacade::Mock(mock) => mock.sched_setaffinity_current(cpuset),
        }
    }

    fn sched_getcpu(&self) -> i32 {
        match self {
            BindingsFacade::Real(bindings) => bindings.sched_getcpu(),
            #[cfg(test)]
            BindingsFacade::Mock(mock) => mock.sched_getcpu(),
        }
    }
}
