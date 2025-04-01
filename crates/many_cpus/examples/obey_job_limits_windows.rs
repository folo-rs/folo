fn main() {
    #[cfg(windows)]
    windows::main().unwrap();

    #[cfg(not(windows))]
    panic!("This example is only supported on Windows.");
}

#[cfg(windows)]
mod windows {
    //! The mechanism used in Windows to enforce limits on processes is Job Objects. Processes are
    //! assigned to jobs, and jobs can be constrained to only use a limited set of processors.
    //!
    //! This example proves that the APIs we offer do not "see" the universe outside of the limits
    //! of the current process's job object constraints.
    //!
    //! If you start the example with no other input, it will start a new instance of itself, assigned
    //! to a job that can only use two processors. The new instance will then sleep for 10 seconds to
    //! allow you to verify that it is properly functioning.
    //!
    //! Job object limits are hard limits, whereas all other mechanisms to define affinity (e.g. CPU
    //! sets and legacy "process affinity masks") are just wishes by the process in question.
    //! In case of conflicting masks, the intersection is used.
    //!
    //! Note that we configure the job object using the legacy affinity mask, which only supports 64
    //! processors. The ProcessorSet API is not limited in this way and can work with any number of
    //! processors. We just use the legacy Windows APIs here to keep the example-specific code simple.
    //! On systems with more than 64 processors, the platform will simply pick one arbitrary processor
    //! group and use two processors from it.

    use many_cpus::ProcessorSet;
    use scopeguard::defer;
    use std::{mem, ptr, thread, time::Duration};
    use windows::{
        Win32::{
            Foundation::CloseHandle,
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_AFFINITY,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                    SetInformationJobObject,
                },
                Threading::{
                    CREATE_SUSPENDED, CreateProcessW, GetCurrentProcess, GetProcessAffinityMask,
                    PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
                },
            },
        },
        core::{PCWSTR, Result},
    };

    pub fn main() -> Result<()> {
        let (process_affinity, system_affinity) = get_current_legacy_process_and_system_affinity()?;

        let processors_present_on_system = system_affinity.count_ones();
        if processors_present_on_system <= 2 {
            println!(
                "System has only {} processors - this example requires at least 3 processors.",
                processors_present_on_system
            );
            return Ok(());
        }

        let process_processor_count = process_affinity.count_ones();

        if process_processor_count <= 2 {
            // This process has been limited - it is the stage 2 of the example.
            println!(
                "This process is allowed to use {process_processor_count} of {processors_present_on_system} processors. Success!"
            );

            // Verify that the ProcessorSet also only sees the two we have been assigned.
            let processor_count = ProcessorSet::all().len();
            assert_eq!(processor_count, process_processor_count as usize);

            thread::sleep(Duration::from_secs(10)); // Keep it alive to observe
        } else {
            println!(
                "This process is not limited to specific processors. Creating restricted instance of process...",
            );
            restart_with_job_limits()?;
        }

        Ok(())
    }

    // We use the job object legacy affinity control mechanisms, so might as well go legacy all the way.
    fn get_current_legacy_process_and_system_affinity() -> Result<(usize, usize)> {
        // SAFETY: No safety requirements. Does not need to be closed.
        let current_process = unsafe { GetCurrentProcess() };

        // What processors we are allowed to use.
        let mut process_affinity = 0;

        // What processors exist.
        let mut system_affinity = 0;

        // SAFETY: No safety requirements, as long as we pass valid inputs.
        unsafe {
            GetProcessAffinityMask(current_process, &mut process_affinity, &mut system_affinity)?;
        }

        Ok((process_affinity, system_affinity))
    }

    fn restart_with_job_limits() -> Result<()> {
        // SAFETY: No safety requirements.
        let job = unsafe { CreateJobObjectW(None, None)? };
        assert!(!job.is_invalid());

        defer! {
            // SAFETY: No safety requirements.
            unsafe {
                CloseHandle(job).unwrap();
            }
        }

        // Set job CPU affinity to processors 0 and 1 (mask = 0x3).
        // Note that this is a legacy affinity configuration, which is limited to 64 processors.
        // This is good enough for our example, as we want to limit to only two anyway, and sticking
        // to the legacy configuration keeps the code here simple and straightforward.
        let mut limit_info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limit_info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_AFFINITY;
        limit_info.BasicLimitInformation.Affinity = 0x3; // Processors 0 and 1

        // SAFETY: No safety requirements, as long as we pass valid inputs.
        unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limit_info).cast(),
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
        }

        // Get the path to the current executable as a wide string, so we can start a new instance.
        let exe_path = std::env::current_exe().expect("Failed to get current executable path");
        let exe_path_wide: Vec<u16> = exe_path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0)) // Null-terminated
            .collect();

        // Prepare process creation structures.
        let startup_info = STARTUPINFOW {
            cb: mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };

        let mut process_info = PROCESS_INFORMATION::default();

        // Create the process in a suspended state, so we can immediately assign it to the job.
        // SAFETY: No safety requirements beyond passing valid inputs.
        unsafe {
            CreateProcessW(
                PCWSTR(exe_path_wide.as_ptr()),
                None,             // No command line arguments
                None,             // Process handle not inheritable
                None,             // Thread handle not inheritable
                false,            // No handle inheritance
                CREATE_SUSPENDED, // Start suspended
                None,             // Use parent's environment
                None,             // Use parent's current directory
                &startup_info,
                &mut process_info,
            )?;
        }

        defer! {
            // SAFETY: No safety requirements.
            unsafe {
                CloseHandle(process_info.hProcess).unwrap();
            }

            // SAFETY: No safety requirements.
            unsafe {
                CloseHandle(process_info.hThread).unwrap();
            }
        }

        // Assign the suspended process to the job, so our limits are enforced.
        // SAFETY: No safety requirements.
        unsafe {
            AssignProcessToJobObject(job, process_info.hProcess)?;
        }

        // Resume the process entrypoint thread to start execution.
        // SAFETY: No safety requirements.
        unsafe {
            ResumeThread(process_info.hThread);
        }

        println!("New instance started with affinity limited to processors 0 and 1.");
        Ok(())
    }
}
