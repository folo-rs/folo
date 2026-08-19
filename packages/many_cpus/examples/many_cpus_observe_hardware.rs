//! We inspect the hardware from the perspective of the current process and write every
//! observation to the terminal as a `key: value` line, one observation per line, so the output
//! of two runs can be compared directly.
//!
//! The numbers need not agree with each other. The extent of the ID space is reported
//! separately from the processors the current process may use, and an ID within that space
//! need not name a processor this process can see. Constraining the process - for example by
//! giving it a subset of the machine's processors - is what makes the two diverge, which is
//! exactly what this example exists to reveal.
//!
//! The example terminates immediately, so it is usable as a one-shot probe of a machine.

use std::fmt::Display;

use many_cpus::{Processor, SystemHardware};

fn main() {
    let hw = SystemHardware::current();

    print_counts(hw);
    print_processor_ids(hw);
    print_memory_region_ids(hw);
}

/// Prints how large the machine is, from each of the perspectives the package offers.
fn print_counts(hw: &SystemHardware) {
    println!("max_processor_count: {}", hw.max_processor_count());
    println!("active_processor_count: {}", hw.active_processor_count());
    println!("max_memory_region_count: {}", hw.max_memory_region_count());

    // `all_processors()` ignores the resource quota of the process, `processors()` obeys it,
    // so the two differ when the process is quota-limited but not processor-limited.
    println!("all_processors_count: {}", hw.all_processors().len());
    println!("processors_count: {}", hw.processors().len());
}

/// Prints the ID of every processor the current process can use, in ascending order.
///
/// The IDs need not be contiguous - a process constrained to a subset of the machine sees
/// only the IDs of the processors in that subset, with the rest of the ID space missing.
fn print_processor_ids(hw: &SystemHardware) {
    let mut ids = hw
        .all_processors()
        .iter()
        .map(Processor::id)
        .collect::<Vec<_>>();

    ids.sort_unstable();

    println!("all_processor_ids: {}", format_ids(&ids));
}

/// Prints the ID of every memory region that the processors of the current process belong to,
/// in ascending order and without repetition.
fn print_memory_region_ids(hw: &SystemHardware) {
    let mut ids = hw
        .all_processors()
        .iter()
        .map(Processor::memory_region_id)
        .collect::<Vec<_>>();

    ids.sort_unstable();
    ids.dedup();

    println!("all_memory_region_ids: {}", format_ids(&ids));
}

/// Formats a list of IDs as a single line, in a form that survives copy-paste into a shell
/// that expects a comma-separated processor list (e.g. the argument of `taskset -c`).
fn format_ids(ids: &[impl Display]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
