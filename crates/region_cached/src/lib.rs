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
//! # Quick start
//! 
//! This crate provides the `region_cached!` macro that enhances static variables with region-local
//! caching behavior and provides interior mutability via eventually consistent writes.
//! 
//! ```rust
//! use region_cached::region_cached;
//! 
//! region_cached!(static FILTER_KEYS: Vec<String> = vec![
//!     "error".to_string(),
//!     "panic".to_string()
//! ]);
//! 
//! /// Returns true if the log line contains any of the filter keys.
//! fn process_log_line(line: &str) -> bool {
//!     // `.with()` provides an immutable reference to the cached value.
//!     FILTER_KEYS.with(|keys| keys.iter().any(|key| line.contains(key)))
//! }
//! 
//! assert!(!process_log_line("info: all is well"));
//! assert!(process_log_line("error: something failed to be processed"));
//! 
//! // You can use `set()` to publish a new value.
//! FILTER_KEYS.set(vec!["error".to_string(), "panic".to_string(), "warning".to_string()]);
//! ```
//! 
//! See `examples/region_cached_log_filtering.rs` for a more complete example.
//! 
//! # Consistency guarantees
//! 
//! Writes are eventually consistent, with an undefined order of resolving from different threads.
//! Writes from the same thread become visible sequentially on all threads, with the last write from
//! the writing thread winning from among other writes from the same thread.
//! 
//! Writes are immediately visible from the originating thread, with the caveats that:
//! 1. Eventually consistent writes from other threads may be applied at any time, such as between
//!    a write and an immediately following read.
//! 2. A thread, if not pinned, may migrate to a new memory region between the write and read
//!    operations, which invalidates any causal link between the two operations.
//! 
//! In general, you can only have firm expectations about the sequencing of data produced by read
//! operations if the writes are always performed from a single thread.
//! 
//! # API
//! 
//! The macro internally transforms a static variable of type `T` into a static variable of type
//! [`RegionCachedKey<T>`][1]. See the API documentation of this type for more details about available
//! methods.
//! 
//! # Cross-region visibility
//! 
//! This type makes the value visible across memory regions, simply enhancing a static variable
//! with same-region caching to ensure high performance during read and write operations.
//! 
//! The `region_local` crate provides a similar mechanism but limits the visibility of values to
//! only a single memory region - updates do not propagate across region boundaries. This may be
//! a useful alternative if you want unique values per memory region.
//! 
//! [1]: crate::RegionCachedKey

mod block;
pub use block::*;

pub(crate) mod hw_info_client;
pub(crate) mod hw_tracker_client;
