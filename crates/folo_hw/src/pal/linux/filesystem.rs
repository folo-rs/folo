use std::{fmt::Debug, fs};

/// Linux has this funny notion of exposing various OS APIs as a virtual filesystem. This trait
/// abstracts this virtual filesystem to allow it to be mocked.
///
/// The scope of this trait is limited to only the virtual filesystem exposed by the OS. We do not
/// expect to do "real" file I/O in this layer. All I/O is synchronous and blocking because we
/// expect it to hit a fast path in the OS, given the data is never on a real storage device.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Filesystem: Debug + Send + Sync + 'static {
    /// Get the contents of the /proc/cpuinfo file.
    fn get_cpuinfo_contents(&self) -> String;

    /// Get the contents of the /sys/devices/system/node/nr_online_nodes file or `None` if it does
    /// not exist.
    fn get_numa_nr_online_nodes_contents(&self) -> Option<String>;

    /// Get the contents of the /sys/devices/system/node/node{}/cpulist file.
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

    fn get_numa_nr_online_nodes_contents(&self) -> Option<String> {
        fs::read_to_string("/sys/devices/system/node/nr_online_nodes").ok()
    }

    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> String {
        fs::read_to_string(format!(
            "/sys/devices/system/node/node{}/cpulist",
            node_index
        ))
        .expect("failed to read NUMA node cpulist - cannot continue execution")
    }
}
