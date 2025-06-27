//! Demonstrates the effect of Windows job object limits by spinning the CPU.

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
    use std::thread;
    use std::time::Duration;

    use new_zealand::nz;
    use testing::{Job, ProcessorTimePct};
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_IDLE,
    };

    const SLEEP_TIME_SECS: u64 = 10;

    pub(crate) fn main() {
        // Exit early if running in a testing environment
        if std::env::var("IS_TESTING").is_ok() {
            println!("Running in testing mode - exiting immediately to prevent infinite operation");
            return;
        }

        let job = Job::builder()
            .with_processor_count(nz!(8))
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<50>())
            .build();

        println!("Starting with limit of 8 processors and 50% processor time.");

        // We start a bunch of worker threads, enough to saturate a bunch of processors.
        for _ in 0..100 {
            start_spinner();
        }

        thread::sleep(Duration::from_secs(SLEEP_TIME_SECS));

        drop(job);
        println!("Switching to 8 processors and 1% processor time.");

        let job = Job::builder()
            .with_processor_count(nz!(1))
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<1>())
            .build();

        thread::sleep(Duration::from_secs(SLEEP_TIME_SECS));

        drop(job);
        println!("Switching to 1 processor and 80% processor time.");

        let job = Job::builder()
            .with_processor_count(nz!(1))
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<80>())
            .build();

        thread::sleep(Duration::from_secs(SLEEP_TIME_SECS));

        drop(job);
        println!("Switching to 4 processors and 75% processor time.");

        let job = Job::builder()
            .with_processor_count(nz!(4))
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<75>())
            .build();

        thread::sleep(Duration::from_secs(SLEEP_TIME_SECS));

        drop(job);
        println!("All done.");
    }

    fn start_spinner() {
        thread::spawn(|| {
            // Avoid the spinning being troublesome for other threads by lowering thread priority.

            // SAFETY: No safety requirements.
            let current_thread = unsafe { GetCurrentThread() };

            // SAFETY: No safety requirements.
            unsafe {
                SetThreadPriority(current_thread, THREAD_PRIORITY_IDLE).unwrap();
            }

            // Spin spin spin spin.

            let mut i: usize = 0;

            loop {
                i = i.wrapping_add(1);
            }
        });
    }
}
