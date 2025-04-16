//! We inspect every processor and write a human-readable description of it to the terminal.
//!
//! This obeys the resource quota assigned to the current process (which is the default behavior).

use many_cpus::ProcessorSet;

fn main() {
    for processor in ProcessorSet::all().processors() {
        println!("{processor:?}");
    }
}
