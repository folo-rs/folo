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
//! Think of this as an equivalent of [`thread_local!`][2], except operating on the memory
//! region boundary instead of the thread boundary.
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # Usage
//!
//! There are two ways to create region-local values:
//!
//! 1. Define a static variable in a [`region_local!`][2] block.
//! 2. Use the [`RegionLocal`][5] type inside a [`PerThread`][4] wrapper.
//!
//! The difference is only a question of convenience - static variables are easier to use but come
//! with language-driven limitations, such as needing to know in advance how many you need and
//! defining them in the code. In contrast, `PerThread<RegionLocal<T>>` is more flexible and
//! you can create any number of instances at runtime, at a cost of having to manually deliver
//! instances to the right place in the code.
//!
//! ## Usage via static variables
//!
//! This crate provides the [`region_local!`][3] macro that enhances static variables with
//! region-local storage behavior and provides interior mutability via weakly consistent
//! writes within the same memory region.
//!
//! ```rust
//! // RegionLocalExt provides required extension methods on region-local
//! // static variables, such as `with_local()` and `set_local()`.
//! use region_local::{region_local, RegionLocalExt};
//!
//! region_local!(static FAVORITE_COLOR: String = "blue".to_string());
//!
//! FAVORITE_COLOR.with_local(|color| {
//!     println!("My favorite color is {color}");
//! });
//!
//! FAVORITE_COLOR.set_local("red".to_string());
//! ```
//!
//! ## Usage via `PerThread<RegionLocal<T>>`
//!
//! There exist situations where a static variable is not suitable. For example, the number of
//! different region-local objects may be determined at runtime (e.g. a separate value
//! for each log source loaded from configuration).
//!
//! In this case, you can directly use the [`RegionLocal`][4] type which underpins the mechanisms
//! exposed by the macro. This type is implemented using the [linked object pattern][3] and
//! is most conveniently used via the [`PerThread<T>`][4] type, as `PerThread<RegionLocal<T>>`.
//!
//! ```rust
//! use linked::PerThread;
//! use region_local::RegionLocal;
//!
//! let favorite_color_regional = PerThread::new(RegionLocal::new("blue".to_string()));
//!
//! // This localizes the variable, identifying the memory region specified storage.
//! let favorite_color = favorite_color_regional.local();
//!
//! favorite_color.with_local(|color| {
//!     println!("My favorite color is {color}");
//! });
//!
//! favorite_color.set_local("red".to_string());
//! ```
//!
//! See the documentation of the [`linked`][linked] crate for more details on the mechanisms
//! offered by the linked object pattern. Additional capabilities exist beyond those described here.
//!
//! # Consistency guarantees
//! [consistency-guarantees]: [#consistency-guarantees]
//!
//! Writes are weakly consistent within the same memory region, with an undefined order of resolving
//! from different threads. Writes from the same thread become visible sequentially on all threads in
//! the same memory region.
//!
//! Writes are immediately visible from the originating thread, with the caveats that:
//! 1. Writes from other threads may be applied at any time, such as between
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
//! The macro internally transforms a static variable of type `T` to a different type and
//! provides additional API surface via extension methods on [`RegionLocalExt<T>`][1].
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
//! Example of using this crate with processor-pinned threads (`examples/region_local_1gb.rs`):
//!
//! ```
#![doc = source_file!("examples/region_local_1gb.rs")]
//! ```
//!
//! # Cross-region visibility
//!
//! The [`region_cached`][5] crate provides a similar mechanism that also publishes the value to all
//! memory regions instead of keeping it region-local. This may be a useful alternative if you do
//! not need to have separate variables per memory region but still want the efficiency benefits
//! of reading from local memory.
//!
//! [1]: crate::RegionLocalExt
//! [2]: https://doc.rust-lang.org/std/macro.thread_local.html
//! [3]: crate::region_local
//! [4]: crate::RegionLocal
//! [5]: https://docs.rs/region_cached/latest/region_cached/

use include_doc::source_file;
use simple_mermaid::mermaid;

mod clients;
mod macros;
mod region_local;
mod region_local_ext;

pub(crate) use clients::*;
pub use region_local::*;
pub use region_local_ext::*;

/// Macros require these things to be public but they are not part of the public API.
#[doc(hidden)]
pub mod __private;
