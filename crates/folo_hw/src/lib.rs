#![allow(dead_code, unused_imports, unused_variables)]

pub mod pinned_threads;

mod identifiers;
pub use identifiers::*;

mod processors;
pub use processors::*;

pub(crate) mod hardware;

#[cfg(test)]
mod tests {
    use ::windows::Win32::System::Threading::{
        GetActiveProcessorCount, GetActiveProcessorGroupCount, GetMaximumProcessorCount,
        GetMaximumProcessorGroupCount,
    };

    use super::*;

    #[test]
    fn it_works() {
        // These APIs give us some high level figures about count of groups and processors.
        // There is no information about processor numbers, NUMA or cache relationships.
        // The groups are the building blocks - basically, any processor that belongs in
        // a different group may have an attribute be different. However, in practice this
        // is not always the case (e.g. the author's split L3 cache package is in one group).
        let max_group_count = unsafe { GetMaximumProcessorGroupCount() };
        let active_group_count = unsafe { GetActiveProcessorGroupCount() };

        println!("max_group_count: {}", max_group_count);
        println!("active_group_count: {}", active_group_count);

        for group_number in 0..active_group_count {
            let max_processor_count = unsafe { GetMaximumProcessorCount(group_number) };
            let active_processor_count = unsafe { GetActiveProcessorCount(group_number) };

            // Processor numbers are actually just relative to the group, there is no global
            // processor number, so it is always a pair of (group_number, processor_number)
            // where the processor number is in the range of 0..active_processor_count.
            println!("group_number: {}", group_number);
            println!("max_processor_count: {}", max_processor_count);
            println!("active_processor_count: {}", active_processor_count);
        }
    }
}
