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
