use std::num::NonZeroU32;

use itertools::Itertools;
use nonempty::{nonempty, NonEmpty};

use crate::pal::{
    linux::{cpulist, Bindings, BuildTargetBindings, BuildTargetFilesystem, Filesystem},
    EfficiencyClass, MemoryRegionIndex, Platform, ProcessorGlobalIndex, ProcessorImpl,
};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

/// Singleton instance of `BuildTargetPlatform`, used by public API types
/// to hook up to the correct PAL implementation.
pub(crate) static BUILD_TARGET_PLATFORM: BuildTargetPlatform =
    BuildTargetPlatform::new(&BuildTargetBindings, &BuildTargetFilesystem);

/// The platform that matches the crate's build target.
///
/// You would only use a different platform in unit tests that need to mock the platform.
/// Even then, whenever possible, unit tests should use the real platform for maximum realism.
#[derive(Debug)]
pub(crate) struct BuildTargetPlatform<
    B: Bindings = BuildTargetBindings,
    FS: Filesystem = BuildTargetFilesystem,
> {
    bindings: &'static B,
    fs: &'static FS,
}

impl<B: Bindings, FS: Filesystem> Platform for BuildTargetPlatform<B, FS> {
    type Processor = ProcessorImpl;

    fn get_all_processors(&self) -> NonEmpty<Self::Processor> {
        self.get_all()
    }

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<Self::Processor>,
    {
        todo!()
    }
}

impl<B: Bindings, FS: Filesystem> BuildTargetPlatform<B, FS> {
    pub(crate) const fn new(bindings: &'static B, fs: &'static FS) -> Self {
        Self { bindings, fs }
    }

    fn get_all(&self) -> NonEmpty<ProcessorImpl> {
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
        // We need to combine two sources of information.
        // 1. /proc/cpuinfo gives us the set of processors available.
        // 2. /sys/devices/system/node/node*/cpulist gives us the processors in each NUMA node.
        // Note: /sys/devices/system/node may be missing if there is only one NUMA node.
        let cpu_infos = self.get_cpuinfo();
        let numa_nodes = self.get_numa_nodes();

        // If we did not get any NUMA node info, construct an imaginary NUMA node containing all.
        let numa_nodes = numa_nodes.unwrap_or_else(|| {
            let all_processor_indexes = (0..cpu_infos.len() as ProcessorGlobalIndex).collect_vec();
            nonempty![NonEmpty::from_vec(all_processor_indexes)
                .expect("must have at least one processor")]
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
                .enumerate()
                .find_map(|(node, node_processors)| {
                    if node_processors.contains(&info.index) {
                        return Some(node);
                    }

                    None
                })
                .expect("processor not found in any NUMA node");

            let efficiency_class = if info.frequency_mhz < max_frequency {
                EfficiencyClass::Efficiency
            } else {
                EfficiencyClass::Performance
            };

            ProcessorImpl {
                index: info.index,
                memory_region_index: memory_region as MemoryRegionIndex,
                efficiency_class,
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
                            "processor" => index = value.parse::<ProcessorGlobalIndex>().ok(),
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
    fn get_numa_nodes(&self) -> Option<NonEmpty<NonEmpty<ProcessorGlobalIndex>>> {
        let node_count = self.fs.get_numa_nr_online_nodes_contents()?;

        let node_count = node_count
            .parse::<NonZeroU32>()
            .expect("NUMA node count was not a number");

        Some(
            NonEmpty::from_vec(
                (0..node_count.get())
                    .map(|node| {
                        let cpulist = self.fs.get_numa_node_cpulist_contents(node);
                        cpulist::parse(&cpulist)
                    })
                    .collect_vec(),
            )
            .expect("we already verified that node count is non-zero"),
        )
    }
}

// One result from /proc/cpuinfo.
#[derive(Debug)]
struct CpuInfo {
    index: ProcessorGlobalIndex,

    /// CPU frequency, rounded to nearest integer. We use this to identify efficiency versus
    /// performance cores, where the processors with max frequency are considered performance
    /// cores and any with lower frequency are considered efficiency cores.
    frequency_mhz: u32,
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use crate::pal::linux::{MockBindings, MockFilesystem};

    use super::*;

    #[test]
    fn get_all_processors_smoke_test() {
        // We imagine a simple system with 2 physical cores, 4 logical processors, all in a
        // single processor group and a single memory region. Welcome to 2010!
        static BINDINGS: LazyLock<MockBindings> = LazyLock::new(MockBindings::new);
        static FILESYSTEM: LazyLock<MockFilesystem> = LazyLock::new(|| {
            let mut fs = MockFilesystem::new();

            fs.expect_get_cpuinfo_contents().times(1).return_const(
                "
processor       : 0
cpu MHz         : 3400.123
whatever        : 123
other           : ignored

processor       : 1
cpu MHz         : 3400.123
whatever        : 123
other           : ignored

processor       : 2
cpu MHz         : 3400.123

processor       : 3
cpu MHz         : 3400.123
something       : does not matter
                "
                .to_string(),
            );

            fs.expect_get_numa_nr_online_nodes_contents()
                .times(1)
                .return_const("1".to_string());

            fs.expect_get_numa_node_cpulist_contents()
                .withf(|node| *node == 0)
                .times(1)
                .return_const("0,1,2-3".to_string());

            fs
        });

        let platform = BuildTargetPlatform::new(&*BINDINGS, &*FILESYSTEM);

        let processors = platform.get_all_processors();

        // We expect to see 4 logical processors. This API does not care about the physical cores.
        assert_eq!(processors.len(), 4);

        // All processors must be in the same memory region.
        assert_eq!(
            1,
            processors
                .iter()
                .map(|p| p.memory_region_index)
                .dedup()
                .count()
        );

        let p0 = &processors[0];
        assert_eq!(p0.index, 0);
        assert_eq!(p0.memory_region_index, 0);

        let p1 = &processors[1];
        assert_eq!(p1.index, 1);
        assert_eq!(p1.memory_region_index, 0);

        let p2 = &processors[2];
        assert_eq!(p2.index, 2);
        assert_eq!(p2.memory_region_index, 0);

        let p3 = &processors[3];
        assert_eq!(p3.index, 3);
        assert_eq!(p3.memory_region_index, 0);
    }
}
