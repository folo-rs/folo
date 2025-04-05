//! We inspect every processor and write a human-readable description of it to the terminal.

use many_cpus::ProcessorSet;

fn main() {
    for processor in ProcessorSet::all().processors() {
        println!("{processor:?}");
    }
}
