use std::fmt::Debug;

#[cfg(test)]
use std::sync::Arc;

use windows::{
    Win32::System::{
        Kernel::PROCESSOR_NUMBER,
        SystemInformation::{
            GROUP_AFFINITY, LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        },
    },
    core::Result,
};

#[cfg(test)]
use crate::pal::windows::MockBindings;

use crate::pal::windows::{Bindings, BuildTargetBindings};

/// Hide the real/mock bindings choice behind a single type.
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
    fn get_active_processor_count(&self, group_number: u16) -> u32 {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_active_processor_count(group_number),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_active_processor_count(group_number),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_maximum_processor_count(&self, group_number: u16) -> u32 {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_maximum_processor_count(group_number),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_maximum_processor_count(group_number),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_maximum_processor_group_count(&self) -> u16 {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_maximum_processor_group_count(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_maximum_processor_group_count(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_current_processor_number_ex(&self) -> PROCESSOR_NUMBER {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_current_processor_number_ex(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_current_processor_number_ex(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    unsafe fn get_logical_processor_information_ex(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()> {
        unsafe {
            match self {
                BindingsFacade::Real(bindings) => bindings.get_logical_processor_information_ex(
                    relationship_type,
                    buffer,
                    returned_length,
                ),
                #[cfg(test)]
                BindingsFacade::Mock(bindings) => bindings.get_logical_processor_information_ex(
                    relationship_type,
                    buffer,
                    returned_length,
                ),
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_numa_highest_node_number(&self) -> u32 {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_numa_highest_node_number(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_numa_highest_node_number(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_current_process_default_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY> {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_current_process_default_cpu_set_masks(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_current_process_default_cpu_set_masks(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_current_thread_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY> {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_current_thread_cpu_set_masks(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_current_thread_cpu_set_masks(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn set_current_thread_cpu_set_masks(&self, masks: &[GROUP_AFFINITY]) {
        match self {
            BindingsFacade::Real(bindings) => bindings.set_current_thread_cpu_set_masks(masks),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.set_current_thread_cpu_set_masks(masks),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_current_job_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY> {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_current_job_cpu_set_masks(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_current_job_cpu_set_masks(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_current_thread_legacy_group_affinity(&self) -> GROUP_AFFINITY {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_current_thread_legacy_group_affinity(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_current_thread_legacy_group_affinity(),
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
