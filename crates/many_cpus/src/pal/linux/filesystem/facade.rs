use std::fmt::Debug;

#[cfg(test)]
use std::sync::Arc;

use crate::pal::linux::{BuildTargetFilesystem, Filesystem};

#[cfg(test)]
use crate::pal::linux::MockFilesystem;

/// Enum to hide the different filesystem implementations behind a single wrapper type.
#[derive(Clone)]
pub(crate) enum FilesystemFacade {
    Real(&'static BuildTargetFilesystem),

    #[cfg(test)]
    Mock(Arc<MockFilesystem>),
}

impl FilesystemFacade {
    pub const fn real() -> Self {
        FilesystemFacade::Real(&BuildTargetFilesystem)
    }

    #[cfg(test)]
    pub fn from_mock(mock: MockFilesystem) -> Self {
        FilesystemFacade::Mock(Arc::new(mock))
    }
}

impl Filesystem for FilesystemFacade {
    fn get_cpuinfo_contents(&self) -> String {
        match self {
            FilesystemFacade::Real(filesystem) => filesystem.get_cpuinfo_contents(),
            #[cfg(test)]
            FilesystemFacade::Mock(mock) => mock.get_cpuinfo_contents(),
        }
    }

    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> String {
        match self {
            FilesystemFacade::Real(filesystem) => {
                filesystem.get_numa_node_cpulist_contents(node_index)
            }
            #[cfg(test)]
            FilesystemFacade::Mock(mock) => mock.get_numa_node_cpulist_contents(node_index),
        }
    }

    fn get_cpu_online_contents(&self, cpu_index: u32) -> Option<String> {
        match self {
            FilesystemFacade::Real(filesystem) => filesystem.get_cpu_online_contents(cpu_index),
            #[cfg(test)]
            FilesystemFacade::Mock(mock) => mock.get_cpu_online_contents(cpu_index),
        }
    }

    fn get_numa_node_possible_contents(&self) -> Option<String> {
        match self {
            FilesystemFacade::Real(filesystem) => filesystem.get_numa_node_possible_contents(),
            #[cfg(test)]
            FilesystemFacade::Mock(mock) => mock.get_numa_node_possible_contents(),
        }
    }

    fn get_proc_self_status_contents(&self) -> String {
        match self {
            FilesystemFacade::Real(filesystem) => filesystem.get_proc_self_status_contents(),
            #[cfg(test)]
            FilesystemFacade::Mock(mock) => mock.get_proc_self_status_contents(),
        }
    }
}

impl Debug for FilesystemFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Mock(inner) => inner.fmt(f),
        }
    }
}