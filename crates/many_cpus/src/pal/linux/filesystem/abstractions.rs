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
    /// NB! This file also includes offline processors. To check if a processor is online, you must
    /// look in /sys/devices/system/cpu/cpu*/online (which has either 0 and 1 as content).
    ///
    /// This is a plaintext file with "key    : value" pairs, blocks separated by empty lines.
    fn get_cpuinfo_contents(&self) -> String;

    /// Get the contents of the /sys/devices/system/node/possible file or `None` if it does
    /// not exist.
    ///
    /// This list all NUMA nodes that could possibly exist in the system, even those that are
    /// offline.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_numa_node_possible_contents(&self) -> Option<String>;

    /// Get the contents of the /sys/devices/system/node/node{}/cpulist file.
    ///
    /// This is a cpulist format file ("0,1,2-4,5-10:2" style list).
    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> String;

    /// Gets the contents of the /sys/devices/system/cpu/cpu{}/online file.
    ///
    /// This is a single line file with either 0 or 1 as content (+ newline).
    /// This file may be absent on some Linux flavors, in which case we assume every CPU is online.
    fn get_cpu_online_contents(&self, cpu_index: u32) -> Option<String>;

    /// Gets the contents of the /prod/{pid}/status file for the current process.
    ///
    /// This is a plaintext file with "key:     value" pairs.
    fn get_proc_self_status_contents(&self) -> String;

    /// Gets the relative path of the cgroup the current process belongs to (e.g. `/foo/bar`)
    /// or `None` if no cgroup is assigned.
    ///
    /// This is a plaintest file with one line for each (sub)process visible to the process.
    ///
    /// ```text
    /// 17:cpuset:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
    /// 16:cpu:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
    /// 15:memory:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
    /// 0::/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
    /// ```
    /// 
    /// This file may contain lines in both cgroups v1 and v2 format. To maintain implementation
    /// sanity, we are going to assume the cgroup name is the same between v1 and v2 and only look
    /// for the v2 line (even if we try using the v1 API to access it later).
    fn get_proc_self_cgroup_name(&self) -> Option<String>;

    /// Gets the cgroup CPU quota and period for the given cgroup name.
    /// 
    /// Probes both v1 and v2 cgroup APIs and returns data from the highest version available.
    /// Returns `None` if the cgroup does not exist or if a limit is not set.
    fn get_cgroup_cpu_quota_and_period_us(&self, name: &str) -> Option<(u64, u64)>;
}
