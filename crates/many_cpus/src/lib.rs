// We disable a bunch of incompatible lint rules under Miri, which leaves dead code around.
#![cfg_attr(miri, allow(unused_imports, unused_variables, dead_code))]

//! Working on many-processor systems with 100+ logical processors can require you to pay extra
//! attention to the specifics of the hardware to make optimal use of available compute capacity
//! and extract the most performance out of the system.
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # Why should one care?
//!
//! Modern operating systems try to distribute work fairly between all processors. Typical Rust
//! sync and async task runtimes like Rayon and Tokio likewise try to be efficient in occupying all
//! processors with work, even moving work between processors if one risks becoming idle. This is fine
//! but we can do better.
//!
//! Taking direct control over the placement of work on specific processors can yield superior
//! performance by taking advantage of factors under the service author's control, which are not known
//! to general-purpose tasking runtimes:
//!
//! 1. A key insight we can use is that most service apps exist to process requests or execute jobs - each
//!    unit of work being done is related to a specific data set. We can ensure we only process the data
//!    associated with a specific HTTP/gRPC request on a single processor to ensure optimal data locality.
//!    This means the data related to the request is likely to be in the caches of that processor, speeding
//!    up all operations related to that request by avoiding expensive memory accesses.
//! 1. Even when data is intentionally shared across processors (e.g. because one processor is not capable
//!    enough to do the work and parallelization is required), performance differences exist between
//!    different pairs of processors because different processors can be connected to different physical
//!    memory modules. Access to non-cached data is optimal when that data is in the same memory region
//!    as the current processor (i.e. on the physical memory modules directly wired to the current
//!    processor).
//!
//! # How does this crate help?
//!
//! The `many_cpus` crate provides mechanisms to schedule threads on specific processors and in specific
//! memory regions, ensuring that work assigned to those threads remains on the same hardware and that
//! data shared between threads is local to the same memory region, enabling you to achieve high data
//! locality and processor cache efficiency.
//!
//! In addition to thread spawning, this crate enables app logic to observe what processor the current
//! thread is executing on and in which memory region this processor is located, even if the thread is
//! not bound to a specific processor. This can be a building block for efficiency improvements even
//! outside directly controlled work scheduling.
//!
//! Other crates from the [Folo project](https://github.com/folo-rs/folo) build upon this hardware-
//! awareness functionality to provide higher-level primitives such as thread pools, work schedulers,
//! region-local cells and more.
//!
//! # Supported operating systems
//!
//! This crate aims to be compatible with:
//!
//! * Windows 11 and newer
//! * Windows Server 2022 and newer
//! * Ubuntu 24.04 and newer
//!
//! # Quick start: spawn threads on specific processors
//!
//! The simplest scenario is when you want to start a thread on every processor
//! (`examples/spawn_on_all_processors.rs`):
//!
//! ```rust
//! use many_cpus::ProcessorSet;
//!
//! let all_threads = ProcessorSet::all().spawn_threads(|processor| {
//!     println!("Spawned thread on processor {}", processor.id());
//!
//!     // In a real service, you would start some work handler here, e.g. to read
//!     // and process messages from a channel or to spawn a web handler.
//! });
//!
//! for thread in all_threads {
//!     thread.join().unwrap();
//! }
//!
//! println!("All threads have finished.");
//! ```
//!
//! # Quick start: define processor selection criteria
//!
//! Depending on the specific circumstances, you may want to filter the set of processors. For example,
//! you may want to use only two processors but ensure that they are high-performance processors that
//! are connected to the same physical memory modules so they can cooperatively perform some processing
//! on a shared data set (`examples/spawn_on_selected_processors.rs`):
//!
//! ```rust
//! use std::num::NonZero;
//!
//! use many_cpus::ProcessorSet;
//!
//! const PROCESSOR_COUNT: NonZero<usize> = NonZero::new(2).unwrap();
//!
//! let selected_processors = ProcessorSet::builder()
//!     .same_memory_region()
//!     .performance_processors_only()
//!     .take(PROCESSOR_COUNT)
//!     .expect("could not find required processors on this system");
//!
//! let all_threads = selected_processors.spawn_threads(|processor| {
//!     println!("Spawned thread on processor {}", processor.id());
//!
//!     // In a real service, you would start some work handler here, e.g. to read
//!     // and process messages from a channel or to spawn a web handler.
//! });
//!
//! for thread in all_threads {
//!     thread.join().unwrap();
//! }
//!
//! println!("All threads have finished.");
//! ```
//!
//! # Quick start: inspect the hardware environment
//!
//! Functions are provided to easily inspect the current hardware environment
//! (`examples/observe_processor.rs`):
//!
//! ```rust
//! use many_cpus::{HardwareInfo, HardwareTracker};
//! use std::{thread, time::Duration};
//! 
//! let max_processors = HardwareInfo::max_processor_count();
//! let max_memory_regions = HardwareInfo::max_memory_region_count();
//! println!(
//!     "This system can support up to {max_processors} processors in {max_memory_regions} memory regions"
//! );
//!
//! loop {
//!     let current_processor_id = HardwareTracker::current_processor_id();
//!     let current_memory_region_id = HardwareTracker::current_memory_region_id();
//!
//!     println!(
//!         "Thread executing on processor {current_processor_id} in memory region {current_memory_region_id}"
//!     );
//!
//! # #[cfg(doc)] // Skip the sleep when testing.
//!     thread::sleep(Duration::from_secs(1));
//! # #[cfg(not(doc))] // Break after first iteration when testing.
//! # break;
//! }
//! ```
//!
//! Note that the current processor may change at any time if you are not using threads pinned to
//! specific processors (such as those spawned via `ProcessorSet::spawn_threads()`). Example output:
//!
//! ```text
//! This system can support up to 32 processors in 1 memory regions
//! Thread executing on processor 4 in memory region 0
//! Thread executing on processor 4 in memory region 0
//! Thread executing on processor 12 in memory region 0
//! Thread executing on processor 2 in memory region 0
//! Thread executing on processor 12 in memory region 0
//! Thread executing on processor 0 in memory region 0
//! Thread executing on processor 4 in memory region 0
//! Thread executing on processor 4 in memory region 0
//! ```
//!
//! # Dynamic changes in hardware configuration
//!
//! The crate does not support detecting and responding to changes in the hardware environment at
//! runtime. For example, if the set of active processors changes at runtime (e.g. due to a change in
//! hardware resources or OS configuration) then this change may not be visible to the crate.
//!
//! # External constraints
//!
//! The operating system may define constraints that prohibit the application from using all the available
//! processors (e.g. when the app is containerized and provided limited hardware resources).
//!
//! This crate treats platform constraints as follows:
//!
//! * Hard limits on which processors are allowed are respected - forbidden processors are mostly
//!   ignored by this crate and cannot be used to spawn threads, though such processors are still
//!   accounted for when inspecting hardware information such as "max processor count".
//!   The mechanisms for defining such limits are cgroups on Linux and job objects on Windows.
//!   See `examples/obey_job_limits_windows.rs` for a Windows-specific example.
//! * Soft limits on which processors are allowed are ignored by default - specifying a processor
//!   affinity via `taskset` on Linux, `start.exe /affinity 0xff` on Windows or similar mechanisms
//!   does not affect the set of processors this crate will use by default, though you can opt in to
//!   this (see next chapter).
//!
//! Note that limits on **processor time** are ignored - they are still enforced by the platform (which
//! will force idle periods to keep the process within the limits) but have no effect on which actual
//! processors are available for use. For example, if you configure a processor time limit of 10 seconds
//! per second of real time on a 20-processor system, then at full utilization this crate may use all 20
//! processors but only for a maximum of 0.5 seconds of each second (leaving the processors idle for
//! the remaining 0.5 seconds, potentially smeared around in smaller units over the whole second).
//!
//! # Example: inheriting soft limits on allowed processors
//!
//! While the crate does not by default obey soft limits, you can opt in to these limits by inheriting
//! the allowed processor set in the `main()` entrypoint thread
//! (`examples/spawn_on_inherited_processors.rs`):
//!
//! ```rust
//! use std::{thread, time::Duration};
//!
//! use many_cpus::ProcessorSet;
//!
//! // The set of processors used here can be adjusted via OS mechanisms.
//! //
//! // For example, to select only processors 0 and 1:
//! // Linux: taskset 0x3 target/debug/examples/spawn_on_inherited_processors
//! // Windows: start /affinity 0x3 target/debug/examples/spawn_on_inherited_processors.exe
//!
//! let inherited_processors = ProcessorSet::builder()
//!     .where_available_for_current_thread()
//!     .take_all()
//!     .expect("found no processors usable by the current thread - impossible because the thread is currently running on one");
//!
//! println!(
//!     "The current thread has inherited an affinity to {} processors.",
//!     inherited_processors.len()
//! );
//!
//! let all_threads = inherited_processors.spawn_threads(|processor| {
//!     println!("Spawned thread on processor {}", processor.id());
//!
//!     // In a real service, you would start some work handler here, e.g. to read
//!     // and process messages from a channel or to spawn a web handler.
//! });
//!
//! for thread in all_threads {
//!     thread.join().unwrap();
//! }
//!
//! println!("All threads have finished. Exiting in 10 seconds.");
//!
//! // Give some time to exit, as on Windows using "start" will create a new window that would
//! // otherwise disappear instantly, making it hard to see what happened.
//! # #[cfg(doc)] // Skip the sleep when testing.
//! thread::sleep(Duration::from_secs(10));
//! ```

mod clients;
mod hardware_info;
mod hardware_tracker;
mod primitive_types;
mod processor;
mod processor_set;
mod processor_set_builder;

pub(crate) use clients::*;
pub use hardware_info::*;
pub use hardware_tracker::*;
pub use primitive_types::*;
pub use processor::*;
pub use processor_set::*;
pub use processor_set_builder::*;

pub(crate) mod pal;
