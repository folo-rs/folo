use std::fmt::Debug;

#[cfg(test)]
use std::sync::Arc;

use windows::{
    core::Result,
    Win32::{
        Foundation::{BOOL, HANDLE},
        System::SystemInformation::{
            GROUP_AFFINITY, LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        },
    },
};

#[cfg(test)]
use crate::pal::windows::MockBindings;

use crate::pal::windows::{Bindings, BuildTargetBindings};

/// Hide the real/mock bindings choice behind a single type.
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
    fn get_active_processor_count(&self, group_number: u16) -> u32 {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_active_processor_count(group_number),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_active_processor_count(group_number),
        }
    }

    fn get_maximum_processor_count(&self, group_number: u16) -> u32 {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_maximum_processor_count(group_number),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_maximum_processor_count(group_number),
        }
    }

    fn get_maximum_processor_group_count(&self) -> u16 {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_maximum_processor_group_count(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_maximum_processor_group_count(),
        }
    }

    fn get_current_thread(&self) -> HANDLE {
        match self {
            BindingsFacade::Real(bindings) => bindings.get_current_thread(),
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => bindings.get_current_thread(),
        }
    }

    unsafe fn set_thread_group_affinity(
        &self,
        thread: HANDLE,
        group_affinity: *const GROUP_AFFINITY,
        previous_group_affinity: Option<*mut GROUP_AFFINITY>,
    ) -> BOOL {
        match self {
            BindingsFacade::Real(bindings) => {
                bindings.set_thread_group_affinity(thread, group_affinity, previous_group_affinity)
            }
            #[cfg(test)]
            BindingsFacade::Mock(bindings) => {
                bindings.set_thread_group_affinity(thread, group_affinity, previous_group_affinity)
            }
        }
    }

    unsafe fn get_logical_processor_information_ex(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()> {
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
