//! We inspect every processor and write a human-readable description of it to the terminal.
//!
//! This does not obey the resource quota available to the current process.

use many_cpus::ProcessorSet;

fn main() {
    for processor in ProcessorSet::builder()
        .ignoring_resource_quota()
        .take_all()
        .unwrap()
        .processors()
    {
        println!("{processor:?}");
    }
}
