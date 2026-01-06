//! The mechanism used in Windows to enforce limits on processes is Job Objects. Processes are
//! assigned to jobs, and jobs can be constrained to only use a limited set of processors.
//!
//! This example proves that the APIs we offer do not "see" the universe outside of the limits
//! of the current process's job object constraints on processor affinity (which processors
//! the process is allowed to use).
//!
//! Job object limits are hard limits, whereas all other mechanisms to define affinity (e.g. CPU
//! sets and legacy "process affinity masks") are just wishes by the process in question.
//! In case of conflicting masks, the intersection is used.
//!
//! This example is Windows-only, as job objects are a Windows-specific feature.

fn main() {
    #[cfg(windows)]
    windows::main();

    #[cfg(not(windows))]
    {
        eprintln!("This example is only supported on Windows.");
        std::process::exit(0);
    }
}

#[cfg(windows)]
mod windows {
    use many_cpus::SystemHardware;
    use new_zealand::nz;
    use testing::Job;

    pub(crate) fn main() {
        // Restrict the current process to only use 2 processors.
        let _job = Job::builder().with_processor_count(nz!(2)).build();

        verify_limits_obeyed();
    }

    fn verify_limits_obeyed() {
        // The default processor set obeys all the limits that apply to the current process.
        let processor_count = SystemHardware::current().processors().len();
        println!("Current process is allowed to use {processor_count} processors.");

        assert_eq!(processor_count, 2);
    }
}
