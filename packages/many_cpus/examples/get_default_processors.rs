//! We inspect every processor in the default set and write a
//! human-readable description of it to the terminal.
//!
//! This obeys the operating system enforced processor selection and resource quota constraints
//! assigned to the current process (which is the default behavior).

use many_cpus::ProcessorSet;

fn main() {
    for processor in ProcessorSet::default().processors() {
        println!("{processor:?}");
    }
}
