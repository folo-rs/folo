use std::{fmt::Debug, fs};

#[cfg(test)]
use std::sync::Arc;

/// Linux has this funny notion of exposing various OS APIs as a virtual filesystem. This trait
/// abstracts this virtual filesystem to allow it to be mocked.
///
/// The scope of this trait is limited to only the virtual filesystem exposed by the OS. We do not
/// expect to do "real" file I/O in this layer. All I/O is synchronous and blocking because we
/// expect it to hit a fast path in the OS, given the data is never on a real storage device.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Filesystem: Debug + Send + Sync + 'static {
    /// Get the contents of the /proc/cpuinfo file.
    ///
    /// This is a plaintext file with "key    : value" pairs, blocks separated by empty lines.
    fn get_cpuinfo_contents(&self) -> String;

    /// Get the contents of the /sys/devices/system/node/online file or `None` if it does
    /// not exist.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_numa_node_online_contents(&self) -> Option<String>;

    /// Get the contents of the /sys/devices/system/node/node{}/cpulist file.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> String;
}

/// The virtual filesystem for the real operating system that the build is targeting.
///
/// You would only use different filesystems in PAL unit tests that need to use a mock filesystem.
/// Even then, whenever possible, unit tests should use the real filesystem for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetFilesystem;

impl Filesystem for BuildTargetFilesystem {
    fn get_cpuinfo_contents(&self) -> String {
        fs::read_to_string("/proc/cpuinfo")
            .expect("failed to read /proc/cpuinfo - cannot continue execution")
    }

    fn get_numa_node_online_contents(&self) -> Option<String> {
        fs::read_to_string("/sys/devices/system/node/online").ok()
    }

    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> String {
        fs::read_to_string(format!(
            "/sys/devices/system/node/node{}/cpulist",
            node_index
        ))
        .expect("failed to read NUMA node cpulist - cannot continue execution")
    }
}

/// Enum to hide the different filesystem implementations behind a single wrapper type.
#[derive(Clone, Debug)]
pub(crate) enum FilesystemImpl {
    Real(&'static BuildTargetFilesystem),

    #[cfg(test)]
    Mock(Arc<MockFilesystem>),
}

impl Filesystem for FilesystemImpl {
    fn get_cpuinfo_contents(&self) -> String {
        match self {
            FilesystemImpl::Real(filesystem) => filesystem.get_cpuinfo_contents(),
            #[cfg(test)]
            FilesystemImpl::Mock(mock) => mock.get_cpuinfo_contents(),
        }
    }

    fn get_numa_node_online_contents(&self) -> Option<String> {
        match self {
            FilesystemImpl::Real(filesystem) => filesystem.get_numa_node_online_contents(),
            #[cfg(test)]
            FilesystemImpl::Mock(mock) => mock.get_numa_node_online_contents(),
        }
    }

    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> String {
        match self {
            FilesystemImpl::Real(filesystem) => {
                filesystem.get_numa_node_cpulist_contents(node_index)
            }
            #[cfg(test)]
            FilesystemImpl::Mock(mock) => mock.get_numa_node_cpulist_contents(node_index),
        }
    }
}
