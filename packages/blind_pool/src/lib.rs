//! This package provides [`BlindPool`], a dynamically growing pool of objects that can store
//! objects of any type.
//!
//! It offers automatic memory management with stable memory addresses
//! and efficient typed insertion with automatic value dropping.
//!
//! [`LocalBlindPool`] is a single-threaded variant with reduced overhead and [`RawBlindPool`]
//! offers manual lifetime management for the most performance-sensitive scenarios.
//!
//! # Features
//!
//! - **Type-agnostic memory management**: The pool accepts objects of any sized type.
//! - **Automatic resource management**: Pooled objects are dropped automatically when no longer
//!   referenced.
//! - **Thread-safe and single-threaded variants**: [`BlindPool`] for multi-threaded use,
//!   [`LocalBlindPool`] for single-threaded performance.
//! - **Always pinned**: Memory address remains valid for the entire lifetime of a pooled object.
//! - **Dynamic growth**: Pool capacity grows automatically as needed, shrinks to fit on-demand.
//! - **Efficient allocation**: Uses high density slabs to minimize allocation overhead.
//! - **Stable Rust**: No unstable Rust features required.
//! - **Optional leak detection**: Pool can be configured to panic on drop if values are
//!   still present (primarily relevant for [`RawBlindPool`]).
//!
//! # Example
//!
//! ```rust
//! use blind_pool::BlindPool;
//!
//! // Create a thread-safe pool.
//! let pool = BlindPool::new();
//!
//! // Insert values and get handles.
//! let u64_handle = pool.insert(42_u64);
//! let string_handle = pool.insert("hello".to_string());
//!
//! // Access values through dereferencing.
//! assert_eq!(*u64_handle, 42);
//! assert_eq!(*string_handle, "hello");
//!
//! // Values are automatically cleaned up when handles are dropped.
//! ```
//!
//! For single-threaded use:
//!
//! ```rust
//! use blind_pool::LocalBlindPool;
//!
//! // Create a single-threaded pool (more efficient).
//! let pool = LocalBlindPool::new();
//!
//! let value_handle = pool.insert(vec![1, 2, 3]);
//! assert_eq!(*value_handle, vec![1, 2, 3]);
//! ```
//!
//! For manual resource management:
//!
//! ```rust
//! use blind_pool::RawBlindPool;
//!
//! // Create a pool with manual resource management.
//! let mut pool = RawBlindPool::new();
//!
//! // Insert values of different types into the same pool.
//! let pooled_u64 = pool.insert(42_u64);
//! let pooled_i32 = pool.insert(-123_i32);
//! let pooled_f32 = pool.insert(3.14_f32);
//!
//! // Read data back from the pooled items.
//! let value_u64 = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     *pooled_u64.ptr().as_ref()
//! };
//!
//! let value_i32 = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     *pooled_i32.ptr().as_ref()
//! };
//!
//! assert_eq!(value_u64, 42);
//! assert_eq!(value_i32, -123);
//!
//! // Manual cleanup required.
//! pool.remove(&pooled_u64);
//! pool.remove(&pooled_i32);
//! pool.remove(&pooled_f32);
//! ```

mod cast;
mod constants;
mod local_pool;
mod local_pooled;
mod pool;
mod pooled;
mod raw;
mod raw_builder;

pub use local_pool::*;
pub use local_pooled::*;
// Re-export DropPolicy from opaque_pool simply because we do not need a different one.
pub use opaque_pool::DropPolicy;
// Re-export so we can use it without the consumer needing a reference.
#[doc(hidden)]
pub use paste::paste as __private_paste;
pub use pool::*;
pub use pooled::*;
pub use raw::*;
pub use raw_builder::*;
