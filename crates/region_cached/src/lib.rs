//! On many-processor systems with multiple memory regions, there is an extra cost associated with
//! accessing data in physical memory modules that are in a different memory region than the current
//! processor:
//!
//! * Cross-memory-region loads have higher latency (e.g. 100 ns local versus 200 ns remote).
//! * Cross-memory-region loads have lower throughput (e.g. 50 Gbps local versus 10 Gbps remote).
//!
//! This crate provides the capability to cache frequently accessed shared data sets in the local memory
//! region, speeding up reads when the data is not already in the local processor caches. You can think
//! of it as an extra level of caching between L3 processor caches and main memory.
//!
#![doc = mermaid!("../doc/region_cached.mermaid")]
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # Applicability
//!
//! A positive performance impact may be seen if all of the following conditions are true:
//!
//! 1. The system has multiple memory regions.
//! 2. A shared data set is accessed by processors in different memory regions.
//! 3. The data set is large enough to make it unlikely that it is resident in local processor caches.
//! 4. The data set is small enough to make it affordable to clone into every memory region.
//!
//! As with all performance and efficiency questions, you should use a profiler to measure real impact.
//!
//! # Usage
//!
//! There are two ways to create region-cached values:
//!
//! 1. Define a static variable in a [`region_cached!`][2] block.
//! 2. Use the [`RegionCached`][5] type inside a [`PerThread`][4] wrapper.
//!
//! The difference is only a question of convenience - static variables are easier to use but come
//! with language-driven limitations, such as needing to know in advance how many you need and
//! defining them in the code. In contrast, `PerThread<RegionCached<T>>` is more flexible and
//! you can create any number of instances at runtime, at a cost of having to manually deliver
//! instances to the right place in the code.
//!
//! ## Usage via static variables
//!
//! This crate provides the [`region_cached!`][2] macro that enhances static variables with
//! region-local caching behavior and provides interior mutability via weakly consistent writes.
//!
//! ```
//! // RegionCachedExt provides required extension methods on region-cached
//! // static variables, such as `with_cached()` and `set_global()`.
//! use region_cached::{region_cached, RegionCachedExt};
//!
//! region_cached!(static FILTER_KEYS: Vec<String> = vec![
//!     "error".to_string(),
//!     "panic".to_string()
//! ]);
//!
//! fn process_log_line(line: &str) -> bool {
//!     FILTER_KEYS.with_cached(|keys| keys.iter().any(|key| line.contains(key)))
//! }
//!
//! assert!(!process_log_line("info: all is well"));
//! assert!(process_log_line("error: something failed to be processed"));
//!
//! FILTER_KEYS.set_global(vec![
//!     "error".to_string(),
//!     "panic".to_string(),
//!     "warning".to_string()
//! ]);
//! ```
//!
//! See `examples/region_cached_log_filtering.rs` for a more complete example of using this macro.
//!
//! ## Usage via `PerThread<RegionCached<T>>`
//!
//! There exist situations where a static variable is not suitable. For example, the number of
//! different region-cached objects may be determined at runtime (e.g. a separate set of filter keys
//! for each log source loaded from configuration).
//!
//! In this case, you can directly use the [`RegionCached`][5] type which underpins the mechanisms
//! exposed by the macro. This type is implemented using the [linked object pattern][3] and
//! is most conveniently used via the [`PerThread<T>`][4] type, as `PerThread<RegionCached<T>>`.
//!
//! ```
//! use linked::{PerThread, ThreadLocal};
//! use region_cached::RegionCached;
//!
//! type Filters = Vec<String>;
//!
//! let filter_keys = PerThread::new(RegionCached::new(vec![
//!     "error".to_string(),
//!     "panic".to_string()
//! ]));
//!
//! // Localizes the `PerThread` instance to the current thread, accessing data
//! // in the current thread's active memory region.
//! let filter_keys_local = filter_keys.local();
//!
//! fn process_log_line(line: &str, filter_keys: &ThreadLocal<RegionCached<Filters>>) -> bool {
//!     filter_keys.with_cached(|keys| keys.iter().any(|key| line.contains(key)))
//! }
//!
//! assert!(!process_log_line("info: all is well", &filter_keys_local));
//! assert!(process_log_line("error: something failed to be processed", &filter_keys_local));
//!
//! filter_keys_local.set_global(vec![
//!     "error".to_string(),
//!     "panic".to_string(),
//!     "warning".to_string()
//! ]);
//! ```
//!
//! See `examples/region_cached_log_filtering_no_statics.rs` for a more complete example of
//! dynamically stored region-cached values that do not require static variables.
//!
//! See the documentation of the [`linked`][linked] crate for more details on the mechanisms
//! offered by the linked object pattern. Additional capabilities exist beyond those described here.
//!
//! # Consistency guarantees
//! [consistency-guarantees]: [#consistency-guarantees]
//!
//! Writes are weakly consistent, with an undefined order of resolving from different threads.
//! Writes from the same thread become visible sequentially on all threads.
//!
//! Writes are immediately visible from the originating thread, with the caveats that:
//! 1. Writes from other threads may be applied at any time, such as between
//!    a local write and an immediately following read.
//! 2. A thread, if not pinned, may migrate to a new memory region between the write and read
//!    operations, which invalidates any causal link between the two operations.
//!
//! In general, you can only have firm expectations about the sequencing of data produced by read
//! operations if the writes are always performed from a single thread and reads on region-pinned
//! threads.
//!
//! # API
//!
//! The macro internally transforms a static variable of type `T` to a different type and
//! provides additional API surface via extension methods on [`RegionCachedExt<T>`][1].
//! See the API documentation of this type for more details about available methods.
//!
//! # Operating system compatibility
//!
//! This crate relies on the collaboration between the Rust global allocator and the operating
//! system to allocate memory in the correct memory region. The default configuration in operating
//! systems tends to encourage region-local allocation but this is not guaranteed.
//!
//! Some evidence suggests that on Windows, region-local allocation is only enabled when the threads
//! are pinned to specific processors in specific memory regions. A similar requirement is not known
//! for Linux (at least Ubuntu 24) but this may differ based on the specific OS and configuration.
//! Perform your own measurements to identify the behavior of your system and adjust the application
//! structure accordingly.
//!
//! Example of using this crate with processor-pinned threads (`examples/region_cached_1gb.rs`):
//!
//! ```
#![doc = source_file!("examples/region_cached_1gb.rs")]
//! ```
//!
//! # Cross-region visibility
//!
//! This type makes the value visible across memory regions, enhancing a static variable with
//! region-local caching to ensure low latency and high memory throughput for read operations.
//!
//! The [`region_local`][6] crate provides a similar mechanism but limits the visibility of values
//! to only a single memory region - updates do not propagate across region boundaries. This may be
//! a useful alternative if you want unique values per memory region, similar to `thread_local!`.
//!
//! [1]: crate::RegionCachedExt
//! [2]: crate::region_cached
//! [3]: linked
//! [4]: linked::PerThread
//! [5]: crate::RegionCached
//! [6]: https://docs.rs/region_local/latest/region_local/

use include_doc::source_file;
use simple_mermaid::mermaid;

mod clients;
mod macros;
mod region_cached;
mod region_cached_ext;

pub(crate) use clients::*;
pub use region_cached::*;
pub use region_cached_ext::*;

/// Macros require these things to be public but they are not part of the public API.
#[doc(hidden)]
pub mod __private;
