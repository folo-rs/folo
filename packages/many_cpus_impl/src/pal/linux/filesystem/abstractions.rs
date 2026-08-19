#![cfg_attr(
    test,
    expect(
        clippy::struct_field_names,
        reason = "false positive from automock generated code"
    )
)]

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
    /// Mainstream kernels describe only the online processors here - a processor that goes
    /// offline disappears from the file entirely. Read
    /// [`get_possible_cpus_contents()`][Self::get_possible_cpus_contents] to learn the extent of
    /// the processor ID space and [`get_online_cpus_contents()`][Self::get_online_cpus_contents]
    /// to learn how many processors are online.
    ///
    /// This is a plaintext file with "key    : value" pairs, blocks separated by empty lines.
    fn get_cpuinfo_contents(&self) -> String;

    /// Get the contents of the /sys/devices/system/cpu/possible file or `None` if it does not
    /// exist.
    ///
    /// This lists every processor that could possibly exist in the system, including processors
    /// that are offline and processors that the current process may not use. The kernel fixes
    /// this set at boot, so it does not change over the lifetime of the system.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_possible_cpus_contents(&self) -> Option<String>;

    /// Get the contents of the /sys/devices/system/cpu/online file or `None` if it does not
    /// exist.
    ///
    /// This lists every processor that is currently online, across the whole system and
    /// regardless of which processors the current process may use.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_online_cpus_contents(&self) -> Option<String>;

    /// Get the contents of the /sys/devices/system/node/possible file or `None` if it does
    /// not exist.
    ///
    /// This list all NUMA nodes that could possibly exist in the system, even those that are
    /// offline.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_numa_node_possible_contents(&self) -> Option<String>;

    /// Get the contents of the /sys/devices/system/node/node{}/cpulist file or `None` if it does
    /// not exist.
    ///
    /// A node that holds no online processor publishes an empty list, and a node that was never
    /// onlined at all publishes no such file.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> Option<String>;

    /// Gets the contents of the /sys/devices/system/cpu/cpu{}/online file.
    ///
    /// This is a single line file with either 0 or 1 as content (+ newline).
    /// This file may be absent on some Linux flavors and is absent for processor 0 on kernels
    /// that cannot take that processor offline, in which case we assume the processor is online.
    fn get_cpu_online_contents(&self, cpu_index: u32) -> Option<String>;

    /// Gets the contents of the /prod/{pid}/status file for the current process.
    ///
    /// This is a plaintext file with "key:     value" pairs.
    fn get_proc_self_status_contents(&self) -> String;

    /// Gets the contents of the /proc/self/cgroup file for the current process.
    fn get_proc_self_cgroup(&self) -> Option<String>;

    /// Contents of `/sys/fs/cgroup/cpu/{name}/cpu.cfs_quota_us`.
    fn get_v1_cgroup_cpu_quota(&self, cgroup_name: &str) -> Option<String>;

    /// Contents of `/sys/fs/cgroup/cpu/{name}/cpu.cfs_period_us`.
    fn get_v1_cgroup_cpu_period(&self, cgroup_name: &str) -> Option<String>;

    /// Contents of `/sys/fs/cgroup/{name}/cpu.max`.
    fn get_v2_cgroup_cpu_quota_and_period(&self, cgroup_name: &str) -> Option<String>;
}
