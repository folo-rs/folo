//! We inspect every processor available to the current process and write a
//! human-readable description of it to the terminal.
//!
//! This obeys the operating system enforced processor selection constraints
//! assigned to the current process (which is always the case).
//!
//! However, this does not obey the resource quota available to the current process. This is
//! typically not useful for executing work but may be useful for inspecting available processors.

use many_cpus::SystemHardware;

fn main() {
    for processor in SystemHardware::current().all_processors() {
        println!("{processor:?}");
    }
}
