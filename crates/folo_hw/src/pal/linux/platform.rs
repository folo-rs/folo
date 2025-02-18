use std::{collections::HashMap, mem, sync::OnceLock};

use itertools::Itertools;
use nonempty::NonEmpty;

use crate::{
    cpulist,
    pal::{
        linux::{filesystem::FilesystemFacade, Bindings, BindingsFacade, Filesystem},
        Platform, ProcessorFacade, ProcessorImpl,
    },
    EfficiencyClass, MemoryRegionId, ProcessorId,
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

    fn current_processor_id(&self) -> ProcessorId {
        self.bindings.sched_getcpu() as ProcessorId
    }

    fn max_processor_id(&self) -> ProcessorId {
        self.get_max_processor_id()
    }

    fn max_memory_region_id(&self) -> MemoryRegionId {
        self.get_max_memory_region_id()
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
                    .cloned()
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
        // We need to combine three sources of information.
        // 1. /proc/cpuinfo gives us the set of processors available.
        // 2. /sys/devices/system/node/node*/cpulist gives us the processors in each NUMA node.
        // 3. /sys/devices/system/cpu/cpu*/online says whether a processor is online.
        // Note: /sys/devices/system/node may be missing if there is only one NUMA node.
        let cpu_infos = self.get_cpuinfo();
        let numa_nodes = self.get_numa_nodes();

        // If we did not get any NUMA node info, construct an imaginary NUMA node containing all.
        let numa_nodes = numa_nodes.unwrap_or_else(|| {
            let all_processor_indexes = (0..cpu_infos.len() as ProcessorId).collect_vec();
            let all_processor_indexes = NonEmpty::from_vec(all_processor_indexes)
                .expect("must have at least one processor");
            [(0, all_processor_indexes)].into_iter().collect()
        });

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

            let is_online = self.fs.get_cpu_online_contents(info.index) == "1";

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
                .map(|line| line.trim())
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

                        match key {
                            "processor" => index = value.parse::<ProcessorId>().ok(),
                            "cpu MHz" => {
                                frequency_mhz = value.parse::<f32>().map(|f| f.round() as u32).ok()
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

    // May return None if everything is in a single NUMA node.
    //
    // Otherwise, returns a list of NUMA nodes, where each entry is a list of processor
    // indexes that belong to that node.
    fn get_numa_nodes(&self) -> Option<HashMap<MemoryRegionId, NonEmpty<ProcessorId>>> {
        let node_indexes = cpulist::parse(&self.fs.get_numa_node_possible_contents()?);

        Some(
            node_indexes
                .into_iter()
                .map(|node| {
                    let cpulist = self.fs.get_numa_node_cpulist_contents(node);
                    (node, cpulist::parse(&cpulist))
                })
                .collect(),
        )
    }
}

// One result from /proc/cpuinfo.
#[derive(Debug)]
struct CpuInfo {
    index: ProcessorId,

    /// CPU frequency, rounded to nearest integer. We use this to identify efficiency versus
    /// performance cores, where the processors with max frequency are considered performance
    /// cores and any with lower frequency are considered efficiency cores.
    frequency_mhz: u32,
}

#[cfg(test)]
mod tests {
    use crate::pal::linux::{MockBindings, MockFilesystem};

    use super::*;

    #[test]
    fn get_all_processors_smoke_test() {
        // We imagine a simple system with 2 physical cores, 4 logical processors, all in a
        // single processor group and a single memory region. Welcome to 2010!
        let mut fs = MockFilesystem::new();

        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3],
            [true, true, true, true],
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
    fn two_numa_nodes_efficiency_performance() {
        let mut fs = MockFilesystem::new();
        // Two nodes, each with 2 processors:
        // Node 0 -> [Performance, Efficiency], Node 1 -> [Efficiency, Performance].
        simulate_processor_layout(
            &mut fs,
            [0, 1, 2, 3],
            [true, true, true, true],
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
            [true, true, true, true, true, true, true, true],
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
            [false, false, false, true, true, true],
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
            [true, false, true, false, true, false, true, false],
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
    /// The simulation is valid for one call to get_all_processors_impl().
    fn simulate_processor_layout<const PROCESSOR_COUNT: usize>(
        fs: &mut MockFilesystem,
        processor_index: [ProcessorId; PROCESSOR_COUNT],
        processor_is_active: [bool; PROCESSOR_COUNT],
        memory_region_index: [MemoryRegionId; PROCESSOR_COUNT],
        frequencies_per_processor: [f64; PROCESSOR_COUNT],
    ) {
        // Remember that the cpuinfo list will return all processors, including inactive ones.

        let mut cpuinfo = String::new();

        for (processor_index, frequency) in
            processor_index.iter().zip(frequencies_per_processor.iter())
        {
            cpuinfo.push_str(&format!("processor       : {processor_index}\n"));
            cpuinfo.push_str(&format!("cpu MHz         : {frequency}\n"));
            cpuinfo.push_str("whatever        : 123\n");
            cpuinfo.push_str("other           : ignored\n");
            cpuinfo.push('\n');
        }

        let node_indexes =
            NonEmpty::from_vec(memory_region_index.iter().copied().unique().collect_vec())
                .expect("simulating zero nodes is not supported");
        let node_indexes_cpulist = cpulist::emit(node_indexes);

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

        for processor_id in processor_index {
            let is_online = processor_is_active[processor_id as usize];
            fs.expect_get_cpu_online_contents()
                .withf(move |p| *p == processor_id)
                .times(1)
                .return_const(if is_online { "1" } else { "0" });
        }

        for (node, processors) in processors_per_node {
            let cpulist = processors
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");

            fs.expect_get_numa_node_cpulist_contents()
                .withf(move |n| *n == node)
                .times(1)
                .return_const(cpulist);
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
        simulate_processor_layout(&mut fs, [0], [true], [0], [2000.0]);

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
        simulate_processor_layout(&mut fs, [0, 1], [true, true], [0, 0], [2000.0, 2000.0]);

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
        simulate_processor_layout(&mut fs, [0, 1], [true, true], [0, 1], [2000.0, 2000.0]);

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
            [true, true, true, true],
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
}
