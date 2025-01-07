use windows::Win32::System::Threading::{
    GetActiveProcessorCount, GetActiveProcessorGroupCount, GetMaximumProcessorCount,
    GetMaximumProcessorGroupCount,
};

fn main() {
    // Neither of these considers any startup-time affinity assignment/limitation. If there is
    // some affinity assigned on startup (e.g. via "start.exe /affinity 0xf") then this still
    // shows non-affine processors are active and individual threads may override startup affinity.
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
