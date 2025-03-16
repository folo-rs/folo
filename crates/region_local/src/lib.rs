//! On many-processor systems with multiple memory regions, there is an extra cost associated with
//! accessing data in physical memory modules that are in a different memory region than the current
//! processor:
//!
//! * Cross-memory-region loads have higher latency (e.g. 100 ns local versus 200 ns remote).
//! * Cross-memory-region loads have lower throughput (e.g. 50 Gbps local versus 10 Gbps remote).
//!
//! This crate provides the capability to create static variables that maintain separate storage per
//! memory region. This may be useful in circumstances where state needs to be shared but only within
//! each memory region (e.g. because you intentionally want to avoid the overhead of cross-memory-region
//! transfers and want to isolate the data sets).
//!
#![doc = mermaid!("../doc/region_local.mermaid")]
//!
//! Think of this as an equivalent of `thread_local!`, except operating on the memory region boundary
//! instead of the thread boundary.
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # Quick start
//!
//! This crate provides the `region_local!` macro that enhances static variables with region-local
//! storage behavior and provides interior mutability via eventually consistent writes within the same
//! memory region.
//!
//! ```rust
//! use region_local::region_local;
//!
//! region_local!(static FAVORITE_COLOR: String = "blue".to_string());
//!
//! FAVORITE_COLOR.with(|color| {
//!     println!("My favorite color is {color}");
//! });
//!
//! FAVORITE_COLOR.set("red".to_string());
//! ```
//!
//! # Consistency guarantees
//!
//! Writes are eventually consistent within the same memory region, with an undefined order of resolving
//! from different threads. Writes from the same thread become visible sequentially on all threads in
//! the same memory region.
//!
//! Writes are immediately visible from the originating thread, with the caveats that:
//! 1. Eventually consistent writes from other threads may be applied at any time, such as between
//!    a write and an immediately following read.
//! 2. A thread, if not pinned, may migrate to a new memory region between the write and read
//!    operations, which invalidates any link between the two operations and reads from the storage
//!    of the new memory region.
//!
//! In general, you can only have firm expectations about the sequencing of data produced by read
//! operations if the writes are always performed from a single thread per memory region and the
//! thread is pinned to processors of only a single memory region.
//!
//! # API
//!
//! The macro internally transforms a static variable of type `T` into a static variable of type
//! [`RegionLocalStatic<T>`][1]. See the API documentation of this type for more details about
//! available methods.
//!
//! # Cross-region visibility
//!
//! The `region_cached` crate provides a similar mechanism that also publishes the value to all
//! memory regions instead of keeping it region-local. This may be a useful alternative if you do
//! not need to have separate variables per memory region but still want the efficiency benefits
//! of reading from local memory.
//!
//! [1]: crate::RegionLocalStatic

use simple_mermaid::mermaid;

mod block;
pub use block::*;

pub(crate) mod hw_info_client;
pub(crate) mod hw_tracker_client;
