use std::fmt::Debug;

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
