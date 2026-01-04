use std::fmt::Debug;
use std::fs;

use crate::pal::linux::Filesystem;

/// The virtual filesystem for the real operating system that the build is targeting.
///
/// You would only use different filesystems in PAL unit tests that need to use a mock filesystem.
/// Even then, whenever possible, unit tests should use the real filesystem for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetFilesystem;

// Real filesystem bindings are excluded from coverage measurement because:
// 1. They are tested via integration tests running on actual Linux.
// 2. Some paths (like cgroups v1) are not available on all test systems.
#[cfg_attr(coverage_nightly, coverage(off))]
impl Filesystem for BuildTargetFilesystem {
    fn get_cpuinfo_contents(&self) -> String {
        fs::read_to_string("/proc/cpuinfo")
            .expect("failed to read /proc/cpuinfo - cannot continue execution")
    }

    fn get_numa_node_possible_contents(&self) -> Option<String> {
        fs::read_to_string("/sys/devices/system/node/possible").ok()
    }

    fn get_numa_node_cpulist_contents(&self, node_index: u32) -> String {
        fs::read_to_string(format!("/sys/devices/system/node/node{node_index}/cpulist",))
            .expect("failed to read NUMA node cpulist - cannot continue execution")
    }

    fn get_cpu_online_contents(&self, cpu_index: u32) -> Option<String> {
        fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu_index}/online")).ok()
    }

    fn get_proc_self_status_contents(&self) -> String {
        fs::read_to_string("/proc/self/status")
            .expect("failed to read /proc/self/status - cannot continue execution")
    }

    fn get_proc_self_cgroup(&self) -> Option<String> {
        fs::read_to_string("/proc/self/cgroup").ok()
    }

    fn get_v1_cgroup_cpu_quota(&self, cgroup_name: &str) -> Option<String> {
        fs::read_to_string(format!("/sys/fs/cgroup/cpu/{cgroup_name}/cpu.cfs_quota_us")).ok()
    }

    fn get_v1_cgroup_cpu_period(&self, cgroup_name: &str) -> Option<String> {
        fs::read_to_string(format!(
            "/sys/fs/cgroup/cpu/{cgroup_name}/cpu.cfs_period_us"
        ))
        .ok()
    }

    fn get_v2_cgroup_cpu_quota_and_period(&self, cgroup_name: &str) -> Option<String> {
        fs::read_to_string(format!("/sys/fs/cgroup/{cgroup_name}/cpu.max")).ok()
    }
}
