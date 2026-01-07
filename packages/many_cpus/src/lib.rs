#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

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
//! # How does this package help?
//!
//! The `many_cpus` package provides mechanisms to schedule threads on specific processors and in specific
//! memory regions, ensuring that work assigned to those threads remains on the same hardware and that
//! data shared between threads is local to the same memory region, enabling you to achieve high data
//! locality and processor cache efficiency.
//!
//! In addition to thread spawning, this package enables app logic to observe what processor the current
//! thread is executing on and in which memory region this processor is located, even if the thread is
//! not bound to a specific processor. This can be a building block for efficiency improvements even
//! outside directly controlled work scheduling.
//!
//! Other packages from the [Folo project](https://github.com/folo-rs/folo) build upon this hardware-
//! awareness functionality to provide higher-level primitives such as thread pools, work schedulers,
//! region-local cells and more.
//!
//! # Quick start
//!
//! The simplest scenario is when you want to start a thread on every processor in the default
//! processor set:
//!
//! ```rust
//! // examples/spawn_on_all_processors.rs
//! # use many_cpus::SystemHardware;
//! let threads = SystemHardware::current()
//!     .processors()
//!     .spawn_threads(|processor| {
//!         println!("Spawned thread on processor {}", processor.id());
//!
//!         // In a real service, you would start some work handler here, e.g. to read
//!         // and process messages from a channel or to spawn a web handler.
//!     });
//! # for thread in threads {
//! #    thread.join().unwrap();
//! # }
//! ```
//!
//! If there are no operating system enforced constraints active, the default processor set
//! includes all processors.
//!
//! # Selection criteria
//!
//! Depending on the specific circumstances, you may want to filter the set of processors.
//! For example, you may want to use only two processors but ensure that they are high-performance
//! processors that are connected to the same physical memory modules so they can cooperatively
//! perform some processing on a shared data set:
//!
//! ```rust
//! // examples/spawn_on_selected_processors.rs
//! # use many_cpus::SystemHardware;
//! # use new_zealand::nz;
//! let hardware = SystemHardware::current();
//!
//! let selected_processors = hardware
//!     .processors()
//!     .to_builder()
//!     .same_memory_region()
//!     .performance_processors_only()
//!     .take(nz!(2))
//!     // If we do not have what we want, we fall back to the default set.
//!     .unwrap_or_else(|| hardware.processors());
//!
//! let threads = selected_processors.spawn_threads(|processor| {
//!     println!("Spawned thread on processor {}", processor.id());
//!
//!     // In a real service, you would start some work handler here, e.g. to read
//!     // and process messages from a channel or to spawn a web handler.
//! });
//! # for thread in threads {
//! #    thread.join().unwrap();
//! # }
//! ```
//!
//! # Inspecting the hardware environment
//!
//! Functions are provided to easily inspect the current hardware environment:
//!
//! ```rust
//! // examples/observe_processor.rs
//! # use many_cpus::SystemHardware;
//! # use std::{thread, time::Duration};
//! let hardware = SystemHardware::current();
//!
//! let max_processors = hardware.max_processor_count();
//! let max_memory_regions = hardware.max_memory_region_count();
//! println!(
//!     "This system can support up to {max_processors} processors in {max_memory_regions} memory regions"
//! );
//!
//! loop {
//!     let current_processor_id = hardware.current_processor_id();
//!     let current_memory_region_id = hardware.current_memory_region_id();
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
#![doc = include_str!("../docs/snippets/external_constraints.md")]
//!
#![doc = include_str!("../docs/snippets/changes_at_runtime.md")]
//!
//! # Inheriting soft limits on allowed processors
//!
//! While the package does not by default obey soft limits, you can opt in to these limits by
//! inheriting the allowed processor set in the `main()` entrypoint thread:
//!
//! ```rust
//! // examples/spawn_on_inherited_processors.rs
//! # use std::{thread, time::Duration};
//! # use many_cpus::SystemHardware;
//! let hardware = SystemHardware::current();
//!
//! // The set of processors used here can be adjusted via OS mechanisms.
//! //
//! // For example, to select only processors 0 and 1:
//! // Linux: `taskset 0x3 target/debug/examples/spawn_on_inherited_processors`
//! // Windows: `start /affinity 0x3 target/debug/examples/spawn_on_inherited_processors.exe`
//! let inherited_processors = hardware
//!     .processors()
//!     .to_builder()
//!     // This causes soft limits on processor affinity to be respected.
//!     .where_available_for_current_thread()
//!     .take_all()
//!     .expect("found no processors usable by the current thread; \
//!         this is impossible because the thread is currently running on one");
//!
//! println!(
//!     "After applying soft limits, we are allowed to use {} processors.",
//!     inherited_processors.len()
//! );
//!
//! let threads = inherited_processors.spawn_threads(|processor| {
//!     println!("Spawned thread on processor {}", processor.id());
//!
//!     // In a real service, you would start some work handler here, e.g. to read
//!     // and process messages from a channel or to spawn a web handler.
//! });
//! # for thread in threads {
//! #    thread.join().unwrap();
//! # }
//! ```
//!
//! # Testing with fake hardware
//!
//! The `many_cpus` package provides a fake hardware capability for testing code that depends on
//! hardware configuration. This is available when the `test-util` Cargo feature is enabled.
//!
//! To make your code testable with fake hardware, accept `SystemHardware` as a value (typically
//! as a function parameter or struct field) instead of always calling `SystemHardware::current()`.
//! This allows tests to substitute fake hardware while production code uses real hardware.
//!
//! See the [`fake`] module for detailed examples and API documentation.
//!
//! # Operating system compatibility
//!
//! This package is tested on the following operating systems:
//!
//! * Windows 11 and newer
//! * Windows Server 2022 and newer
//! * Ubuntu 24.04 and newer
//!
//! The functionality may also work on other operating systems if they offer compatible platform
//! APIs but this is not actively tested.
//!
//! ## Unsupported platforms
//!
//! On operating systems without native support (such as macOS, BSD variants, etc.), this package
//! provides a fallback implementation that allows code to compile and run with graceful degradation:
//!
//! * Processor count is determined via `std::thread::available_parallelism()`
//! * All processors are simulated as being in a single memory region (region 0)
//! * All processors are marked as Performance class
//! * Thread pinning operations succeed but do not actually pin threads to processors
//! * Current processor tracking uses stable thread-local IDs derived from thread IDs
//!
//! While this fallback behavior maintains API compatibility and allows applications to function,
//! it does not provide the performance benefits of actual processor pinning and topology awareness.
//! Applications running on unsupported platforms will not see performance improvements from using
//! this package but will still function correctly.

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/folo-rs/folo/refs/heads/main/packages/many_cpus/icon.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/folo-rs/folo/refs/heads/main/packages/many_cpus/icon.ico"
)]

mod primitive_types;
mod processor;
mod processor_set;
mod processor_set_builder;
mod resource_quota;
mod system_hardware;

#[cfg(any(test, feature = "test-util"))]
pub mod fake;

pub use primitive_types::*;
pub use processor::*;
pub use processor_set::*;
pub use processor_set_builder::*;
pub use resource_quota::*;
pub use system_hardware::SystemHardware;

// No documented public API but we have benchmarks that reach in via undocumented private API.
#[doc(hidden)]
pub mod pal;
