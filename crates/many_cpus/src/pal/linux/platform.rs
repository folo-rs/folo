use std::{iter::once, mem, sync::OnceLock};

use foldhash::HashMap;
use itertools::Itertools;
use nonempty::NonEmpty;

use crate::{
    EfficiencyClass, MemoryRegionId, ProcessorId,
    pal::{
        Platform, ProcessorFacade, ProcessorImpl,
        linux::{Bindings, BindingsFacade, Filesystem, filesystem::FilesystemFacade},
    },
};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

/// Singleton instance of `BuildTargetPlatform`, used by public API types
/// to hook up to the correct PAL implementation.
pub(crate) static BUILD_TARGET_PLATFORM: BuildTargetPlatform =
    BuildTargetPlatform::new(BindingsFacade::real(), FilesystemFacade::real());

/// The platform that matches the crate's build target.
///
/// You would only use a different platform in unit tests that need to mock the platform.
/// Even then, whenever possible, unit tests should use the real platform for maximum realism.
#[derive(Debug)]
pub(crate) struct BuildTargetPlatform {
    bindings: BindingsFacade,
    fs: FilesystemFacade,

    // Including inactive.
    all_processors: OnceLock<NonEmpty<ProcessorImpl>>,
    max_processor_id: OnceLock<ProcessorId>,
    max_memory_region_id: OnceLock<MemoryRegionId>,

    // Only active.
    all_active_processors: OnceLock<NonEmpty<ProcessorFacade>>,
}

impl Platform for BuildTargetPlatform {
    fn get_all_processors(&self) -> NonEmpty<ProcessorFacade> {
        self.get_active_processors().clone()
    }

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>,
    {
        // SAFETY: Zero-initialized cpu_set_t is a valid value.
        let mut cpu_set: libc::cpu_set_t = unsafe { mem::zeroed() };

        for processor in processors.iter() {
            // SAFETY: No safety requirements.
            unsafe {
                // TODO: This can go out of bounds with giant CPU set (1000+), we would need to use
                // dynamically allocated CPU sets instead of relying on the fixed-size one in libc.
                libc::CPU_SET(processor.as_ref().as_real().id as usize, &mut cpu_set);
            }
        }

        self.bindings
            .sched_setaffinity_current(&cpu_set)
            .expect("failed to configure thread affinity");
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "negative processor IDs are not valid regardless, we do not expect to receive them"
    )]
    fn current_processor_id(&self) -> ProcessorId {
        self.bindings.sched_getcpu() as ProcessorId
    }

    fn max_processor_id(&self) -> ProcessorId {
        self.get_max_processor_id()
    }

    fn max_memory_region_id(&self) -> MemoryRegionId {
        self.get_max_memory_region_id()
    }

    fn current_thread_processors(&self) -> NonEmpty<ProcessorId> {
        let max_processor_id = self.get_max_processor_id();

        let affinity = self
            .bindings
            .sched_getaffinity_current()
            .expect("failed to get current thread processor affinity");

        NonEmpty::from_vec(
            (0..=max_processor_id)
                // TODO: Do we need to check for cpuset overflow here to avoid panic?
                // SAFETY: No safety requirements.
                .filter(|processor_id| unsafe { libc::CPU_ISSET(*processor_id as usize, &affinity) })
                .collect_vec())
                .expect("current thread has no processors in its affinity mask - impossible because this code is running on an active processor")
    }

    fn max_processor_time(&self) -> f64 {
        // This is our ceiling - we cannot use more processor time than the number of processors.
        #[expect(
            clippy::cast_precision_loss,
            reason = "all realistic values are in safe bounds"
        )]
        let max_processor_time = self.get_all_processors().len() as f64;

        // If we are constrained by a cgroup, the ceiling may be lowered.
        if let Some(cgroup_max_processor_time) = self.cgroups_max_processor_time() {
            // We are allowed to use at most the minimum of the two.
            return max_processor_time.min(cgroup_max_processor_time);
        }

        max_processor_time
    }

    fn active_processor_count(&self) -> usize {
        self.get_active_processors().len()
    }
}

impl BuildTargetPlatform {
    pub(super) const fn new(bindings: BindingsFacade, fs: FilesystemFacade) -> Self {
        Self {
            bindings,
            fs,
            all_processors: OnceLock::new(),
            all_active_processors: OnceLock::new(),
            max_processor_id: OnceLock::new(),
            max_memory_region_id: OnceLock::new(),
        }
    }

    fn get_all_processors_impl(&self) -> &NonEmpty<ProcessorImpl> {
        self.all_processors
            .get_or_init(|| self.load_all_processors())
    }

    fn get_active_processors(&self) -> &NonEmpty<ProcessorFacade> {
        self.all_active_processors.get_or_init(|| {
            NonEmpty::from_vec(
                self.get_all_processors_impl()
                    .iter()
                    .filter(|p| p.is_active)
                    .copied()
                    .map(ProcessorFacade::Real)
                    .collect_vec())
                    .expect("found 0 active processors - impossible because this code is running on an active processor")
        })
    }

    fn get_max_memory_region_id(&self) -> MemoryRegionId {
        *self.max_memory_region_id.get_or_init(|| {
            self.get_all_processors_impl()
                .iter()
                .map(|p| p.memory_region_id)
                .max()
                .expect("NonEmpty always has at least one item")
        })
    }

    fn get_max_processor_id(&self) -> ProcessorId {
        *self.max_processor_id.get_or_init(|| {
            self.get_all_processors_impl()
                .iter()
                .map(|p| p.id)
                .max()
                .expect("NonEmpty always has at least one item")
        })
    }

    fn load_all_processors(&self) -> NonEmpty<ProcessorImpl> {
        // There are two main ways to get processor information on Linux:
        // 1. Use various APIs to get the information as objects.
        // 2. Parse files in the /sys and /proc virtual filesystem.
        //
        // The former is "nicer" but requires more code and annoying FFI calls and working with
        // native Linux libraries, which is always troublesome because there is often a klunky
        // extra layer between the operating system and the app (e.g. libnuma, libcpuset, ...).
        //
        // To keep things simple, we will go with the latter.
        //
        // We need to combine multiple sources of information.
        // 1. /proc/cpuinfo gives us the set of processors available.
        // 2. /sys/devices/system/node/node*/cpulist gives us the processors in each NUMA node.
        // 3. /sys/devices/system/cpu/cpu*/online says whether a processor is online.
        // 4. /proc/self/status gives us the set of processors allowed for the current process.
        // Note: /sys/devices/system/node may be missing if there is only one NUMA node.
        let cpu_infos = self.get_cpuinfo();
        let numa_nodes = self.get_numa_nodes();
        let allowed_processors = self.get_processors_allowed_for_current_process();

        // Just filter out disallowed processors right away.
        let cpu_infos = NonEmpty::from_vec(cpu_infos
            .into_iter()
            .filter(|info| allowed_processors.contains(&info.index))
            .collect_vec()).expect("found no allowed processors after filtering out forbidden processors - so how is this code even executing?");

        // If we did not get any NUMA node info, construct an imaginary NUMA node containing all.
        let numa_nodes = numa_nodes
            .unwrap_or_else(|| once((0, cpu_infos.clone().map(|info| info.index))).collect());

        // We identify efficiency cores by comparing the frequency of each processor to the maximum
        // frequency of all processors. If the frequency is less than the maximum, we consider it an
        // efficiency core.
        let max_frequency = cpu_infos
            .iter()
            .map(|info| info.frequency_mhz)
            .max()
            .expect("must have at least one processor in NonEmpty");

        let mut processors = cpu_infos.map(|info| {
            let memory_region = numa_nodes
                .iter()
                .find_map(|(node, node_processors)| {
                    if node_processors.contains(&info.index) {
                        return Some(*node);
                    }

                    None
                })
                .expect("processor not found in any NUMA node");

            let efficiency_class = if info.frequency_mhz < max_frequency {
                EfficiencyClass::Efficiency
            } else {
                EfficiencyClass::Performance
            };

            // Some Linux flavors do not report this, so just assume online by default.
            // Sometimes this is also omitted for a specific processor because... it just is.
            let is_online = self
                .fs
                .get_cpu_online_contents(info.index)
                .is_none_or(|s| s.trim() == "1");

            ProcessorImpl {
                id: info.index,
                memory_region_id: memory_region,
                efficiency_class,
                is_active: is_online,
            }
        });

        // We must return the processors sorted by global index. While the above logic may
        // already ensure this as a side-effect, we will sort here explicitly to be sure.
        processors.sort();

        processors
    }

    fn get_cpuinfo(&self) -> NonEmpty<CpuInfo> {
        let cpuinfo = self.fs.get_cpuinfo_contents();
        let lines = cpuinfo.lines();

        // Process groups of lines delimited by empty lines.
        NonEmpty::from_vec(
            lines
                .map(str::trim)
                .chunk_by(|l| l.is_empty())
                .into_iter()
                .filter_map(|(is_empty, lines)| {
                    if is_empty {
                        return None;
                    }

                    // This line gives us the processor index:
                    // processor       : 29
                    //
                    // This line gives us the processor frequency:
                    // cpu MHz         : 3400.036
                    //
                    // All other lines we ignore.

                    let mut index = None;
                    let mut frequency_mhz = None;

                    for line in lines {
                        let (key, value) = line
                            .split_once(':')
                            .map(|(key, value)| (key.trim(), value.trim()))
                            .expect("/proc/cpuinfo line was not a key:value pair");

                        #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation, reason = "we expect small positive numbers for frequency, which can have their integer part losslessly converted to u32")]
                        match key {
                            "processor" => index = value.parse::<ProcessorId>().ok(),
                            "cpu MHz" => {
                                frequency_mhz = value.parse::<f32>().map(|f| f.round() as u32).ok();
                            }
                            _ => {}
                        }
                    }

                    Some(CpuInfo {
                        index: index.expect("processor index not found for processor"),
                        frequency_mhz: frequency_mhz
                            .expect("processor frequency not found for processor"),
                    })
                })
                .collect_vec(),
        )
        .expect("must have at least one processor in /proc/cpuinfo to function")
    }

    fn get_processors_allowed_for_current_process(&self) -> NonEmpty<ProcessorId> {
        // On Linux, mechanisms like cgroups may limit what processors we are allowed to use.
        // Attempting to pin a thread to forbidden processors will fail. We want to avoid even
        // showing such processors, so we filter them out. The allowed list is in /proc/.../status.

        let status = self.fs.get_proc_self_status_contents();
        let lines = status.lines();

        let cpus_allowed_list = lines
            .into_iter()
            .map(str::trim)
            .filter_map(|line| {
                if line.is_empty() {
                    // There do not seem to be empty lines in this file but just in case.
                    return None;
                }

                // Example content:
                // Speculation_Store_Bypass:       thread vulnerable
                // SpeculationIndirectBranch:      conditional enabled
                // Cpus_allowed:   ffffffff
                // Cpus_allowed_list:      0-31
                // Mems_allowed:   1
                // Mems_allowed_list:      0
                // voluntary_ctxt_switches:        3
                // nonvoluntary_ctxt_switches:     0

                let (key, value) = line
                    .split_once(':')
                    .map(|(key, value)| (key.trim(), value.trim()))
                    .expect("/proc/self/status line was not a key:value pair");

                if key == "Cpus_allowed_list" {
                    return Some(value);
                }

                None
            })
            .take(1)
            .collect_vec();

        let cpus_allowed_list = cpus_allowed_list
            .first()
            .expect("Cpus_allowed_list not found in /proc/self/status");

        NonEmpty::from_vec(
            cpulist::parse(cpus_allowed_list)
                .expect("platform provided invalid cpulist in Cpus_allowed_list"),
        )
        .expect(
            "platform provided empty cpulist in Cpus_allowed_list - at least one must be allowed",
        )
    }

    // May return None if everything is in a single NUMA node.
    //
    // Otherwise, returns a list of NUMA nodes, where each entry is a list of processor
    // indexes that belong to that node.
    fn get_numa_nodes(&self) -> Option<HashMap<MemoryRegionId, NonEmpty<ProcessorId>>> {
        let node_indexes = cpulist::parse(self.fs.get_numa_node_possible_contents()?.trim())
            .expect("platform provided invalid cpulist for list of NUMA nodes");

        Some(
            node_indexes
                .into_iter()
                .map(|node| {
                    let cpulist_str = self.fs.get_numa_node_cpulist_contents(node);
                    let cpulist = NonEmpty::from_vec(
                        cpulist::parse(cpulist_str.trim())
                            .expect("platform provided invalid cpulist for NUMA node members"))
                        .expect("platform provided empty cpulist for NUMA node members - at least one processor must be present to make a NUMA node");

                    (node, cpulist)
                })
                .collect(),
        )
    }

    /// Processor time limit in processor-seconds per second.
    fn cgroups_max_processor_time(&self) -> Option<f64> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "unavoidable but also unlikely since typical values will be in safe bounds"
        )]
        self.fs
            .get_proc_self_cgroup()
            .and_then(parse_cgroup_name)
            .and_then(|name| {
                self.get_cgroup_cpu_quota_and_period_us(&name)
                    .map(|(quota, period)| {
                        let quota = quota as f64;
                        let period = period as f64;

                        // If there is a zero in either field, we just accept what the platform is
                        // telling us. It is nonsense but if the platform gives us nonsense, we
                        // should eat it. A conversion down the line will probably convert this
                        // to an integer count of processors (if used), which will be 0 either way
                        // as NaN is converted to 0 on integer conversion. This 0 will presumbly
                        // signal an error along the lines of "you cannot have 0 processors". Not
                        // worth spending our code and tests on such bizarre lies from the platform.
                        quota / period
                    })
            })
    }

    /// Gets the cgroup CPU quota and period for the given cgroup name.
    ///
    /// Probes both v1 and v2 cgroup APIs and returns data from the highest version available.
    /// Returns `None` if the cgroup does not exist or if a limit is not set.
    fn get_cgroup_cpu_quota_and_period_us(&self, name: &str) -> Option<(u64, u64)> {
        self.get_v2_cgroup_cpu_quota_and_period_us(name)
            .or_else(|| self.get_v1_cgroup_cpu_quota_and_period_us(name))
    }

    fn get_v2_cgroup_cpu_quota_and_period_us(&self, name: &str) -> Option<(u64, u64)> {
        let contents = self.fs.get_v2_cgroup_cpu_quota_and_period(name)?;
        parse_v2_cgroup_cpu_quota_and_period_us(&contents)
    }

    fn get_v1_cgroup_cpu_quota_and_period_us(&self, name: &str) -> Option<(u64, u64)> {
        let quota_contents = self.fs.get_v1_cgroup_cpu_quota(name)?;
        let period_contents = self.fs.get_v1_cgroup_cpu_period(name)?;

        parse_v1_cgroup_cpu_quota_and_period_us(&quota_contents, &period_contents)
    }
}

// One result from /proc/cpuinfo.
#[derive(Clone, Debug)]
struct CpuInfo {
    index: ProcessorId,

    /// CPU frequency, rounded to nearest integer. We use this to identify efficiency versus
    /// performance cores, where the processors with max frequency are considered performance
    /// cores and any with lower frequency are considered efficiency cores.
    frequency_mhz: u32,
}

/// This is the relative path of the cgroup the current process belongs to (e.g. `/foo/bar`)
/// or `None` if no cgroup is assigned.
///
/// The content a plaintest file with one line for each (sub)process visible to the process.
///
/// ```text
/// 17:cpuset:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
/// 16:cpu:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
/// 15:memory:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
/// 0::/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
/// ```
///
/// This file may contain lines in both cgroups v1 and v2 format. To maintain implementation
/// simplicity, we are going to assume the cgroup name is the same between v1 and v2 and only look
/// for the v2 line (even if we try using the v1 API to access it later). In all tested
/// configurations so far this has been the case.
fn parse_cgroup_name(cgroup_contents: impl AsRef<str>) -> Option<String> {
    cgroup_contents.as_ref().lines().find_map(|line| {
        if !line.starts_with("0::") {
            return None;
        }

        Some(line.chars().skip(3).collect::<String>())
    })
}

fn parse_v2_cgroup_cpu_quota_and_period_us(contents: &str) -> Option<(u64, u64)> {
    let contents = contents.trim();

    // There are actual rules about what constitutes "unlimited" but we just treat anything
    // that does not successfully parse as "unlimited" because complaining about it will not help.
    let (quota_str, period_str) = contents.split_once(' ')?;
    let quota = quota_str.parse::<u64>().ok()?;
    let period = period_str.parse::<u64>().ok()?;

    Some((quota, period))
}

fn parse_v1_cgroup_cpu_quota_and_period_us(
    quota_contents: &str,
    period_contents: &str,
) -> Option<(u64, u64)> {
    let quota_contents = quota_contents.trim();
    let period_contents = period_contents.trim();

    // There are actual rules about what constitutes "unlimited" but we just treat anything
    // that does not successfully parse as "unlimited" because complaining about it will not help.
    let quota = quota_contents.parse::<u64>().ok()?;
    let period = period_contents.parse::<u64>().ok()?;

    Some((quota, period))
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::indexing_slicing,
    reason = "we need not worry in tests"
)]
#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use testing::f64_diff_abs;

    use crate::pal::linux::{MockBindings, MockFilesystem};

    use super::*;

    const PROCESSOR_TIME_CLOSE_ENOUGH: f64 = 0.01;

    #[test]
    fn get_all_processors_smoke_test() {
        // We imagine a simple system with 2 physical cores, 4 logical processors, all in a
        // single processor group and a single memory region. Welcome to 2010!
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3],
            None,
            None,
            [0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let processors = platform.get_all_processors();

        // We expect to see 4 logical processors. This API does not care about the physical cores.
        assert_eq!(processors.len(), 4);

        // All processors must be in the same memory region.
        assert_eq!(
            1,
            processors
                .iter()
                .map(|p| p.as_real().memory_region_id)
                .dedup()
                .count()
        );

        let p0 = &processors[0];
        assert_eq!(p0.as_real().id, 0);
        assert_eq!(p0.as_real().memory_region_id, 0);

        let p1 = &processors[1];
        assert_eq!(p1.as_real().id, 1);
        assert_eq!(p1.as_real().memory_region_id, 0);

        let p2 = &processors[2];
        assert_eq!(p2.as_real().id, 2);
        assert_eq!(p2.as_real().memory_region_id, 0);

        let p3 = &processors[3];
        assert_eq!(p3.as_real().id, 3);
        assert_eq!(p3.as_real().memory_region_id, 0);
    }

    #[test]
    fn forbidden_processors_are_ignored() {
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3],
            None,
            // We expect processor 2 to be absent from our results.
            Some([true, true, false, true]),
            [0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let processors = platform.get_all_processors();

        assert_eq!(processors.len(), 3);

        // All processors must be in the same memory region.
        assert_eq!(
            1,
            processors
                .iter()
                .map(|p| p.as_real().memory_region_id)
                .dedup()
                .count()
        );

        let p0 = &processors[0];
        assert_eq!(p0.as_real().id, 0);
        assert_eq!(p0.as_real().memory_region_id, 0);

        let p1 = &processors[1];
        assert_eq!(p1.as_real().id, 1);
        assert_eq!(p1.as_real().memory_region_id, 0);

        let p2 = &processors[2];
        assert_eq!(p2.as_real().id, 3);
        assert_eq!(p2.as_real().memory_region_id, 0);
    }

    #[test]
    fn forbidden_memory_regions_are_ignored() {
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3],
            None,
            // Processors 2 and 3 are part of a memory region with 0 allowed processors.
            // We expect this memory region to be completely absent from any sort of results.
            Some([true, true, false, false]),
            [0, 0, 1, 1],
            [99.9, 99.9, 99.9, 99.9],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let processors = platform.get_all_processors();

        assert_eq!(processors.len(), 2);

        // All processors must be in the same memory region.
        assert_eq!(
            1,
            processors
                .iter()
                .map(|p| p.as_real().memory_region_id)
                .dedup()
                .count()
        );

        let p0 = &processors[0];
        assert_eq!(p0.as_real().id, 0);
        assert_eq!(p0.as_real().memory_region_id, 0);

        let p1 = &processors[1];
        assert_eq!(p1.as_real().id, 1);
        assert_eq!(p1.as_real().memory_region_id, 0);
    }

    #[test]
    fn two_numa_nodes_efficiency_performance() {
        let mut fs = MockFilesystem::new();
        // Two nodes, each with 2 processors:
        // Node 0 -> [Performance, Efficiency], Node 1 -> [Efficiency, Performance].
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3],
            None,
            None,
            [0, 0, 1, 1],
            [3400.0, 2000.0, 2000.0, 3400.0],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 4);

        // Node 0
        let p0 = &processors[0];
        assert_eq!(p0.as_real().id, 0);
        assert_eq!(p0.as_real().memory_region_id, 0);
        assert_eq!(p0.as_real().efficiency_class, EfficiencyClass::Performance);

        let p1 = &processors[1];
        assert_eq!(p1.as_real().id, 1);
        assert_eq!(p1.as_real().memory_region_id, 0);
        assert_eq!(p1.as_real().efficiency_class, EfficiencyClass::Efficiency);

        // Node 1
        let p2 = &processors[2];
        assert_eq!(p2.as_real().id, 2);
        assert_eq!(p2.as_real().memory_region_id, 1);
        assert_eq!(p2.as_real().efficiency_class, EfficiencyClass::Efficiency);

        let p3 = &processors[3];
        assert_eq!(p3.as_real().id, 3);
        assert_eq!(p3.as_real().memory_region_id, 1);
        assert_eq!(p3.as_real().efficiency_class, EfficiencyClass::Performance);
    }

    #[test]
    fn one_big_numa_two_small_nodes() {
        let mut fs = MockFilesystem::new();
        // Three nodes: node 0 -> 4 Performance, node 1 -> 2 Efficiency, node 2 -> 2 Efficiency
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5, 6, 7],
            None,
            None,
            [0, 0, 0, 0, 1, 1, 2, 2],
            [
                3400.0, 3400.0, 3400.0, 3400.0, 2000.0, 2000.0, 2000.0, 2000.0,
            ],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 8);

        // First 4 in node 0 => Performance
        for i in 0..4 {
            let p = &processors[i];
            assert_eq!(p.as_real().id, i as ProcessorId);
            assert_eq!(p.as_real().memory_region_id, 0);
            assert_eq!(p.as_real().efficiency_class, EfficiencyClass::Performance);
        }
        // Next 2 in node 1 => Efficiency
        for i in 4..6 {
            let p = &processors[i];
            assert_eq!(p.as_real().id, i as ProcessorId);
            assert_eq!(p.as_real().memory_region_id, 1);
            assert_eq!(p.as_real().efficiency_class, EfficiencyClass::Efficiency);
        }
        // Last 2 in node 2 => Efficiency
        for i in 6..8 {
            let p = &processors[i];
            assert_eq!(p.as_real().id, i as ProcessorId);
            assert_eq!(p.as_real().memory_region_id, 2);
            assert_eq!(p.as_real().efficiency_class, EfficiencyClass::Efficiency);
        }
    }

    #[test]
    fn one_active_one_inactive_numa_node() {
        let mut fs = MockFilesystem::new();
        // Node 0 -> inactive, Node 1 -> [Performance, Efficiency, Performance]
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5],
            Some([false, false, false, true, true, true]),
            None,
            [0, 0, 0, 1, 1, 1],
            [3400.0, 2000.0, 3400.0, 3400.0, 2000.0, 3400.0],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 3);

        // Node 1 => [Perf, Eff, Perf]
        let p0 = &processors[0];
        assert_eq!(p0.as_real().id, 3);
        assert_eq!(p0.as_real().memory_region_id, 1);
        assert_eq!(p0.as_real().efficiency_class, EfficiencyClass::Performance);

        let p1 = &processors[1];
        assert_eq!(p1.as_real().id, 4);
        assert_eq!(p1.as_real().memory_region_id, 1);
        assert_eq!(p1.as_real().efficiency_class, EfficiencyClass::Efficiency);

        let p2 = &processors[2];
        assert_eq!(p2.as_real().id, 5);
        assert_eq!(p2.as_real().memory_region_id, 1);
        assert_eq!(p2.as_real().efficiency_class, EfficiencyClass::Performance);
    }

    #[test]
    fn two_numa_nodes_some_inactive_processors() {
        let mut fs = MockFilesystem::new();
        // Node 0 -> Efficiency, Node 1 -> Performance, with gaps in indexes
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5, 6, 7],
            Some([true, false, true, false, true, false, true, false]),
            None,
            [0, 0, 0, 0, 1, 1, 1, 1],
            [
                2000.0, 2000.0, 2000.0, 2000.0, 3400.0, 3400.0, 3400.0, 3400.0,
            ],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        assert_eq!(processors.len(), 4);

        // Node 0 => [Eff, Eff]
        let p0 = &processors[0];
        assert_eq!(p0.as_real().id, 0);
        assert_eq!(p0.as_real().memory_region_id, 0);
        assert_eq!(p0.as_real().efficiency_class, EfficiencyClass::Efficiency);

        let p1 = &processors[1];
        assert_eq!(p1.as_real().id, 2);
        assert_eq!(p1.as_real().memory_region_id, 0);
        assert_eq!(p1.as_real().efficiency_class, EfficiencyClass::Efficiency);

        // Node 1 => [Perf, Perf]
        let p2 = &processors[2];
        assert_eq!(p2.as_real().id, 4);
        assert_eq!(p2.as_real().memory_region_id, 1);
        assert_eq!(p2.as_real().efficiency_class, EfficiencyClass::Performance);

        let p3 = &processors[3];
        assert_eq!(p3.as_real().id, 6);
        assert_eq!(p3.as_real().memory_region_id, 1);
        assert_eq!(p3.as_real().efficiency_class, EfficiencyClass::Performance);
    }

    /// Configures mock bindings and filesystem to simulate a particular type of processor layout.
    ///
    /// The simulation is valid for one call to `get_all_processors_impl()`.
    fn simulate_processor_layout<const PROCESSOR_COUNT: usize>(
        fs: &mut MockFilesystem,
        processor_index: [ProcessorId; PROCESSOR_COUNT],
        // If None, all are active.
        processor_is_active: Option<[bool; PROCESSOR_COUNT]>,
        // If None, all are allowed.
        processor_is_allowed: Option<[bool; PROCESSOR_COUNT]>,
        memory_region_index: [MemoryRegionId; PROCESSOR_COUNT],
        frequencies_per_processor: [f64; PROCESSOR_COUNT],
    ) {
        let processor_is_active = processor_is_active.unwrap_or([true; PROCESSOR_COUNT]);
        let processor_is_allowed = processor_is_allowed.unwrap_or([true; PROCESSOR_COUNT]);

        // Remember that the cpuinfo list will return all processors, including inactive ones.

        let mut cpuinfo = String::new();

        for (processor_index, frequency) in
            processor_index.iter().zip(frequencies_per_processor.iter())
        {
            writeln!(cpuinfo, "processor       : {processor_index}").unwrap();
            writeln!(cpuinfo, "cpu MHz         : {frequency}").unwrap();
            writeln!(cpuinfo, "whatever        : 123").unwrap();
            writeln!(cpuinfo, "other           : ignored").unwrap();
            writeln!(cpuinfo).unwrap();
        }

        let node_indexes =
            NonEmpty::from_vec(memory_region_index.iter().copied().unique().collect_vec())
                .expect("simulating zero nodes is not supported");
        let mut node_indexes_cpulist = cpulist::emit(node_indexes);
        // \n might or might not be present, so let's verify that it gets trimmed if it is.
        node_indexes_cpulist.push('\n');

        let processors_per_node = memory_region_index
            .iter()
            .copied()
            .zip(processor_index.iter().copied())
            .into_group_map();

        fs.expect_get_cpuinfo_contents()
            .times(1)
            .return_const(cpuinfo);

        fs.expect_get_numa_node_possible_contents()
            .times(1)
            .return_const(Some(node_indexes_cpulist));

        for (index, processor_id) in processor_index.iter().copied().enumerate() {
            if !processor_is_allowed[index] {
                // Forbidden processors are not probed.
                continue;
            }

            let is_online = processor_is_active[processor_id as usize];
            fs.expect_get_cpu_online_contents()
                .withf(move |p| *p == processor_id)
                .times(1)
                .return_const(if is_online {
                    // \n might or might not be present, so let's verify that it gets trimmed if it is.
                    Some("1\n".to_string())
                } else {
                    Some("0".to_string())
                });
        }

        for (node, processors) in processors_per_node {
            let mut cpulist = processors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");

            // This might or might not be present, so let's verify that it gets trimmed if it is.
            cpulist.push('\n');

            fs.expect_get_numa_node_cpulist_contents()
                .withf(move |n| *n == node)
                .times(1)
                .return_const(cpulist);
        }

        let allowed_processors = NonEmpty::from_vec(processor_index
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(index, processor_id)| {
                if processor_is_allowed[index] {
                    Some(processor_id)
                } else {
                    None
                }
            })
            .collect_vec()).expect("simulated configuration allows zero processors - this is not valid, as some processor must be present to execute the code under test");

        let allowed_cpus = cpulist::emit(allowed_processors);

        fs.expect_get_proc_self_status_contents()
            .times(1)
            .return_const(format!("Cpus_allowed_list: {allowed_cpus}"));
    }

    /// Set quota to -1 for infinity (transformed for v2).
    fn simulate_cgroup_time_limit(
        fs: &mut MockFilesystem,
        quota: i64,
        period: i64,
        v1: bool,
        v2: bool,
    ) {
        const CGROUP_NAME: &str = "/foo/bar";

        let cgroup_file_contents = format!(
            "17:cpuset:{CGROUP_NAME}
16:cpu:{CGROUP_NAME}
15:memory:{CGROUP_NAME}
0::{CGROUP_NAME}
"
        );

        fs.expect_get_proc_self_cgroup()
            .times(1)
            .return_const(cgroup_file_contents);

        if v1 {
            fs.expect_get_v1_cgroup_cpu_period()
                .withf(move |name| name == CGROUP_NAME)
                .times(1)
                .return_const(period.to_string());
            fs.expect_get_v1_cgroup_cpu_quota()
                .withf(move |name| name == CGROUP_NAME)
                .times(1)
                .return_const(quota.to_string());
        }

        if v2 {
            if quota == -1 {
                fs.expect_get_v2_cgroup_cpu_quota_and_period()
                    .withf(move |name| name == CGROUP_NAME)
                    .times(1)
                    .return_const("max".to_string());
            } else {
                fs.expect_get_v2_cgroup_cpu_quota_and_period()
                    .withf(move |name| name == CGROUP_NAME)
                    .times(1)
                    .return_const(format!("{quota} {period}"));
            }
        } else {
            // v2 is always checked first, so if only v1 we still
            // need to return None here.
            fs.expect_get_v2_cgroup_cpu_quota_and_period()
                .withf(move |name| name == CGROUP_NAME)
                .times(1)
                .return_const(None);
        }

        // If neither is requested, we also need to return None for v1 quota,
        // as that is probed to check if data v1 is available.
        if !v1 && !v2 {
            fs.expect_get_v1_cgroup_cpu_quota()
                .withf(move |name| name == CGROUP_NAME)
                .times(1)
                .return_const(None);
        }
    }

    #[test]
    fn pin_current_thread_to_single_processor() {
        let mut bindings = MockBindings::new();

        let expected_set = cpuset_from([0]);

        bindings
            .expect_sched_setaffinity_current()
            .withf(move |cpu_set| {
                // SAFETY: No safety requirements.
                unsafe { libc::CPU_EQUAL(cpu_set, &expected_set) }
            })
            .times(1)
            .returning(|_| Ok(()));

        let mut fs = MockFilesystem::new();
        simulate_processor_layout(&mut fs, [0], None, None, [0], [2000.0]);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(bindings),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        platform.pin_current_thread_to(&processors);
    }

    #[test]
    fn pin_current_thread_to_multiple_processors() {
        let mut bindings = MockBindings::new();

        let expected_set = cpuset_from([0, 1]);

        bindings
            .expect_sched_setaffinity_current()
            .withf(move |cpu_set| {
                // SAFETY: No safety requirements.
                unsafe { libc::CPU_EQUAL(cpu_set, &expected_set) }
            })
            .times(1)
            .returning(|_| Ok(()));

        let mut fs = MockFilesystem::new();
        simulate_processor_layout(&mut fs, [0, 1], None, None, [0, 0], [2000.0, 2000.0]);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(bindings),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        platform.pin_current_thread_to(&processors);
    }

    #[test]
    fn pin_current_thread_to_multiple_memory_regions() {
        let mut bindings = MockBindings::new();

        let expected_set = cpuset_from([0, 1]);

        bindings
            .expect_sched_setaffinity_current()
            .withf(move |cpu_set| {
                // SAFETY: No safety requirements.
                unsafe { libc::CPU_EQUAL(cpu_set, &expected_set) }
            })
            .times(1)
            .returning(|_| Ok(()));

        let mut fs = MockFilesystem::new();
        simulate_processor_layout(&mut fs, [0, 1], None, None, [0, 1], [2000.0, 2000.0]);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(bindings),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        platform.pin_current_thread_to(&processors);
    }

    #[test]
    fn pin_current_thread_to_efficiency_processors() {
        let mut bindings = MockBindings::new();

        let expected_set = cpuset_from([1, 2]);

        bindings
            .expect_sched_setaffinity_current()
            .withf(move |cpu_set| {
                // SAFETY: No safety requirements.
                unsafe { libc::CPU_EQUAL(cpu_set, &expected_set) }
            })
            .times(1)
            .returning(|_| Ok(()));

        let mut fs = MockFilesystem::new();
        // Node 0 -> [Performance, Efficiency], Node 1 -> [Efficiency, Performance]
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3],
            None,
            None,
            [0, 0, 2, 2],
            [3400.0, 2000.0, 2000.0, 3400.0],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(bindings),
            FilesystemFacade::from_mock(fs),
        );
        let processors = platform.get_all_processors();
        let efficiency_processors = NonEmpty::from_vec(
            processors
                .iter()
                .filter(|p| p.as_real().efficiency_class == EfficiencyClass::Efficiency)
                .collect_vec(),
        )
        .unwrap();
        platform.pin_current_thread_to(&efficiency_processors);
    }

    fn cpuset_from<const PROCESSOR_COUNT: usize>(
        processors: [ProcessorId; PROCESSOR_COUNT],
    ) -> libc::cpu_set_t {
        // SAFETY: Zero-initialized CPU set is correct.
        let mut cpu_set: libc::cpu_set_t = unsafe { mem::zeroed() };

        for processor in processors {
            // SAFETY: No safety requirements.
            unsafe {
                // TODO: This can go out of bounds with giant CPU set, we need to use dynamically
                // allocated CPU sets instead of relying on the fixed-size one in libc.
                libc::CPU_SET(processor as usize, &mut cpu_set);
            }
        }

        cpu_set
    }

    #[test]
    fn current_thread_processors_smoke_test() {
        let mut bindings = MockBindings::new();

        let expected_set_1 = cpuset_from([0, 1]);
        let expected_set_2 = cpuset_from([2]);

        bindings
            .expect_sched_getaffinity_current()
            .times(1)
            .returning(move || Ok(expected_set_1));

        bindings
            .expect_sched_getaffinity_current()
            .times(1)
            .returning(move || Ok(expected_set_2));

        let mut fs = MockFilesystem::new();
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2],
            None,
            None,
            [0, 0, 0],
            [2000.0, 2000.0, 1000.0],
        );

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(bindings),
            FilesystemFacade::from_mock(fs),
        );

        let current_thread_processors = platform.current_thread_processors();
        assert_eq!(current_thread_processors.len(), 2);
        assert_eq!(current_thread_processors[0], 0);
        assert_eq!(current_thread_processors[1], 1);

        let current_thread_processors = platform.current_thread_processors();
        assert_eq!(current_thread_processors.len(), 1);
        assert_eq!(current_thread_processors[0], 2);
    }

    #[test]
    fn max_processor_time_without_cgroup() {
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5],
            // 2 inactive processors.
            Some([true, true, true, true, false, false]),
            None,
            [0, 0, 0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9, 99.9, 99.9],
        );

        fs.expect_get_proc_self_cgroup().times(1).return_const(None);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let max_processor_time = platform.max_processor_time();

        #[expect(
            clippy::float_cmp,
            reason = "we use absolute error, which is the right way to compare"
        )]
        {
            assert_eq!(
                f64_diff_abs(max_processor_time, 4.0, PROCESSOR_TIME_CLOSE_ENOUGH),
                0.0
            );
        }
    }

    #[test]
    fn max_processor_time_below_available_v1() {
        // If the limit is less than the number of available processors,
        // we should use the cgroup limit as max processor time.
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5],
            // 2 inactive processors.
            Some([true, true, true, true, false, false]),
            None,
            [0, 0, 0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9, 99.9, 99.9],
        );

        simulate_cgroup_time_limit(&mut fs, 20, 10, true, false);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let max_processor_time = platform.max_processor_time();

        #[expect(
            clippy::float_cmp,
            reason = "we use absolute error, which is the right way to compare"
        )]
        {
            assert_eq!(
                f64_diff_abs(max_processor_time, 2.0, PROCESSOR_TIME_CLOSE_ENOUGH),
                0.0
            );
        }
    }

    #[test]
    fn max_processor_time_below_available_v2() {
        // If the limit is less than the number of available processors,
        // we should use the cgroup limit as max processor time.
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5],
            // 2 inactive processors.
            Some([true, true, true, true, false, false]),
            None,
            [0, 0, 0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9, 99.9, 99.9],
        );

        simulate_cgroup_time_limit(&mut fs, 20, 10, false, true);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let max_processor_time = platform.max_processor_time();

        #[expect(
            clippy::float_cmp,
            reason = "we use absolute error, which is the right way to compare"
        )]
        {
            assert_eq!(
                f64_diff_abs(max_processor_time, 2.0, PROCESSOR_TIME_CLOSE_ENOUGH),
                0.0
            );
        }
    }

    #[test]
    fn max_processor_time_above_available() {
        // If the limit is more than the number of available processors,
        // we should use the available processor count as max processor time.
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5],
            // 2 inactive processors.
            Some([true, true, true, true, false, false]),
            None,
            [0, 0, 0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9, 99.9, 99.9],
        );

        simulate_cgroup_time_limit(&mut fs, 99999, 100, false, true);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let max_processor_time = platform.max_processor_time();

        #[expect(
            clippy::float_cmp,
            reason = "we use absolute error, which is the right way to compare"
        )]
        {
            assert_eq!(
                f64_diff_abs(max_processor_time, 4.0, PROCESSOR_TIME_CLOSE_ENOUGH),
                0.0
            );
        }
    }

    #[test]
    fn max_processor_time_with_infinite_limit_v1() {
        // If the limit is infinity,
        // we should use the available processor count as max processor time.
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5],
            // 2 inactive processors.
            Some([true, true, true, true, false, false]),
            None,
            [0, 0, 0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9, 99.9, 99.9],
        );

        simulate_cgroup_time_limit(&mut fs, -1, 100, true, false);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let max_processor_time = platform.max_processor_time();

        #[expect(
            clippy::float_cmp,
            reason = "we use absolute error, which is the right way to compare"
        )]
        {
            assert_eq!(
                f64_diff_abs(max_processor_time, 4.0, PROCESSOR_TIME_CLOSE_ENOUGH),
                0.0
            );
        }
    }

    #[test]
    fn max_processor_time_with_no_limit() {
        // If there is no data in the limit file,
        // we should use the available processor count as max processor time.
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5],
            // 2 inactive processors.
            Some([true, true, true, true, false, false]),
            None,
            [0, 0, 0, 0, 0, 0],
            [99.9, 99.9, 99.9, 99.9, 99.9, 99.9],
        );

        simulate_cgroup_time_limit(&mut fs, 50, 100, false, false);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(MockBindings::new()),
            FilesystemFacade::from_mock(fs),
        );

        let max_processor_time = platform.max_processor_time();

        #[expect(
            clippy::float_cmp,
            reason = "we use absolute error, which is the right way to compare"
        )]
        {
            assert_eq!(
                f64_diff_abs(max_processor_time, 4.0, PROCESSOR_TIME_CLOSE_ENOUGH),
                0.0
            );
        }
    }

    #[test]
    fn parse_cgroup_name_typical() {
        let input =
            "17:cpuset:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
16:cpu:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
15:memory:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
0::/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f";

        let expected = "/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f";

        let result = parse_cgroup_name(input).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn parse_cgroup_name_v2_only() {
        let input = "0::/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f";

        let expected = "/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f";

        let result = parse_cgroup_name(input).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn parse_cgroup_name_v1_only() {
        let input =
            "17:cpuset:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
        16:cpu:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f
        15:memory:/docker/6a74f501e3b4c9d93ad440a7b73149cf2b5d56073c109a8d774c0793f7fe267f";

        let result = parse_cgroup_name(input);

        // We do not today support v1-only configurations. In theory, we could add support for this
        // but we will hold off on supporting a legacy API version until there is a customer need
        // because all test systems use at least a hybrid v1/v2 configuration where even if the data
        // is configured via v1 API, the name is still published via v2 API.
        assert!(result.is_none());
    }

    #[test]
    fn parse_cgroup_name_garbage() {
        let input = "this does not appear to be a valid cgroup file";

        let result = parse_cgroup_name(input);
        assert!(result.is_none());
    }

    #[test]
    fn parse_v2_cgroup_cpu_quota_and_period_us_typical() {
        let input = "100000 100000";
        let expected = (100_000, 100_000);

        let result = parse_v2_cgroup_cpu_quota_and_period_us(input).unwrap();
        assert_eq!(result, expected);

        let input = "3333 1000";
        let expected = (3333, 1000);

        let result = parse_v2_cgroup_cpu_quota_and_period_us(input).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn parse_v2_cgroup_cpu_quota_and_period_us_unlimited() {
        let input = "max";

        let result = parse_v2_cgroup_cpu_quota_and_period_us(input);
        assert!(result.is_none());
    }

    #[test]
    fn parse_v2_cgroup_cpu_quota_and_period_us_garbage() {
        let input = "12345 this is complete garbage";

        let result = parse_v2_cgroup_cpu_quota_and_period_us(input);
        // We treat errors as missing data and ignore it, little point complaining here.
        assert!(result.is_none());
    }

    #[test]
    fn parse_v1_cgroup_cpu_quota_and_period_us_typical() {
        let quota = "100000";
        let period = "100000";
        let expected = (100_000, 100_000);

        let result = parse_v1_cgroup_cpu_quota_and_period_us(quota, period).unwrap();
        assert_eq!(result, expected);

        let quota = "3333";
        let period = "1000";
        let expected = (3333, 1000);

        let result = parse_v1_cgroup_cpu_quota_and_period_us(quota, period).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn parse_v1_cgroup_cpu_quota_and_period_us_unlimited() {
        let quota = "-1";
        let period = "100000";

        let result = parse_v1_cgroup_cpu_quota_and_period_us(quota, period);
        assert!(result.is_none());
    }

    #[test]
    fn parse_v1_cgroup_cpu_quota_and_period_us_garbage() {
        let quota = "this is garbage";
        let period = "there is no data here";

        let result = parse_v1_cgroup_cpu_quota_and_period_us(quota, period);
        // We treat errors as missing data and ignore it, little point complaining here.
        assert!(result.is_none());
    }

    #[test]
    fn basic_facts_are_represented() {
        let mut fs = MockFilesystem::new();

        //  3 memory regions, each containing 3 processors.
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3, 4, 5, 6, 7, 8],
            None,
            None,
            [0, 1, 2, 0, 1, 2, 0, 1, 2],
            [2000.0; 9],
        );

        let mut bindings = MockBindings::new();

        bindings.expect_sched_getcpu().times(1).return_const(5);

        let platform = BuildTargetPlatform::new(
            BindingsFacade::from_mock(bindings),
            FilesystemFacade::from_mock(fs),
        );

        assert_eq!(platform.current_processor_id(), 5);
        assert_eq!(platform.max_processor_id(), 8);
        assert_eq!(platform.max_memory_region_id(), 2);
    }
}
