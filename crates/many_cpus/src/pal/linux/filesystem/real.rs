use std::{fmt::Debug, fs};

use crate::pal::linux::Filesystem;

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

    fn get_proc_self_cgroup_name(&self) -> Option<String> {
        fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|contents| {
                contents.lines().find_map(|line| {
                    if !line.starts_with("0::") {
                        return None;
                    }

                    Some(line.chars().skip(3).collect::<String>())
                })
            })
    }

    fn get_cgroup_cpu_quota_and_period_us(&self, name: &str) -> Option<(u64, u64)> {
        Self::get_v2_cgroup_cpu_quota_and_period_us(name)
            .or_else(|| Self::get_v1_cgroup_cpu_quota_and_period_us(name))
    }
}

impl BuildTargetFilesystem {
    fn get_v2_cgroup_cpu_quota_and_period_us(name: &str) -> Option<(u64, u64)> {
        let path = format!("/sys/fs/cgroup/{name}/cpu.max");

        let contents = fs::read_to_string(&path).ok()?;

        if contents.trim() == "max" {
            return None;
        }

        let (quota_str, period_str) = contents.split_once(' ')?;

        let quota = quota_str.parse::<u64>().ok()?;
        let period = period_str.parse::<u64>().ok()?;

        Some((quota, period))
    }

    fn get_v1_cgroup_cpu_quota_and_period_us(name: &str) -> Option<(u64, u64)> {
        let quota_path = format!("/sys/fs/cgroup/cpu/{name}/cpu.cfs_quota_us");
        let period_path = format!("/sys/fs/cgroup/cpu/{name}/cpu.cfs_period_us");

        let quota_str = fs::read_to_string(&quota_path).ok()?;
        let period_str = fs::read_to_string(&period_path).ok()?;

        if quota_str.trim() == "-1" {
            return None;
        }

        let quota = quota_str.parse::<u64>().ok()?;
        let period = period_str.parse::<u64>().ok()?;

        Some((quota, period))
    }
}
