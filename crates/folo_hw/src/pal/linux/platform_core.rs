use std::{
    fs::{self, read_to_string},
    num::NonZeroU32,
    ops::Range,
};

use itertools::Itertools;
use nonempty::{nonempty, NonEmpty};

use crate::pal::{
    linux::Bindings, EfficiencyClass, MemoryRegionIndex, ProcessorGlobalIndex, ProcessorImpl,
};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

#[derive(Copy, Clone, Debug, Eq, Ord, Hash, PartialEq, PartialOrd)]
pub(super) struct PlatformCore<B: Bindings> {
    bindings: &'static B,
}

impl<B: Bindings> PlatformCore<B> {
    pub(super) const fn new(bindings: &'static B) -> Self {
        Self { bindings }
    }

    pub(super) fn get_all_processors(&self) -> NonEmpty<ProcessorImpl> {
        self.get_all()
    }

    pub(super) fn pin_current_thread_to<P>(&self, _processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorImpl>,
    {
        todo!()
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
        let numa_nodes = numa_nodes
            .unwrap_or_else(|| nonempty![nonempty![0..cpu_infos.len() as ProcessorGlobalIndex]]);

        // We identify efficiency cores by comparing the frequency of each processor to the maximum
        // frequency of all processors. If the frequency is less than the maximum, we consider it an
        // efficiency core.
        let max_frequency = cpu_infos
            .iter()
            .map(|info| info.frequency_mhz)
            .max()
            .expect("must have at least one processor in NonEmpty");

        cpu_infos.map(|info| {
            let memory_region = numa_nodes
                .iter()
                .enumerate()
                .find_map(|(node, cpulist)| {
                    cpulist
                        .iter()
                        .find(|range| range.contains(&info.index))
                        .map(|_| node)
                })
                .expect("processor not found in any NUMA node");

            let efficiency_class = if info.frequency_mhz < max_frequency {
                EfficiencyClass::Efficiency
            } else {
                EfficiencyClass::Performance
            };

            ProcessorImpl {
                index: info.index,
                memory_region: memory_region as MemoryRegionIndex,
                efficiency_class,
            }
        })
    }

    fn get_cpuinfo(&self) -> NonEmpty<CpuInfo> {
        let cpuinfo = read_to_string("/proc/cpuinfo")
            .expect("operating without /proc/cpuinfo is not supported");
        let lines = cpuinfo.lines();

        // Process groups of lines delimited by empty lines.
        NonEmpty::from_vec(
            lines
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
    // Otherwise, returns a list of NUMA nodes, where each entry is a list of processor
    // index ranges that belong to that node. For example:
    // 0: 0-7,16-23,55,67
    // 1: 8-15,24-31,56,68
    fn get_numa_nodes(&self) -> Option<NonEmpty<NonEmpty<Range<ProcessorGlobalIndex>>>> {
        if !fs::exists("/sys/devices/system/node")
            .ok()
            .unwrap_or_default()
        {
            return None;
        }

        let node_count = fs::read_to_string("/sys/devices/system/node/nr_online_nodes").expect(
            "failed to probe system for NUMA node configuration - cannot continue execution",
        );

        let node_count = node_count
            .parse::<NonZeroU32>()
            .expect("NUMA node count was not a number");

        Some(
            NonEmpty::from_vec(
                (0..node_count.get())
                    .map(|node| {
                        let cpulist = read_to_string(format!(
                            "/sys/devices/system/node/node{}/cpulist",
                            node
                        ))
                        .expect("failed to read cpulist file for NUMA node");

                        parse_cpulist(&cpulist)
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

// 0-7,16-23,55,67
// note: this fails to support the stride operator and is overall a very basic parser
fn parse_cpulist(cpulist: &str) -> NonEmpty<Range<ProcessorGlobalIndex>> {
    let parts = cpulist.split(',');

    NonEmpty::from_vec(parts.map(|entry| {
        if let Ok(number) = entry.parse::<ProcessorGlobalIndex>() {
            number..number + 1
        } else {
            let (start, end) = entry
                .split_once('-')
                .map(|(start, end)| {
                    (
                        start.parse::<ProcessorGlobalIndex>().expect("invalid index in cpulist range"),
                        end.parse::<ProcessorGlobalIndex>().expect("invalid index in cpulist range"),
                    )
                })
                .expect("invalid cpulist entry; expected a single number or a range and this is neither");

            start..end + 1
        }
    }).collect_vec()).expect("invalid cpulist; must have at least one entry")
}
