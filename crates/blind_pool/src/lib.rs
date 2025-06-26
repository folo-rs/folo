//! This crate provides [`BlindPool`], a dynamically growing pool of objects that can store
//! objects with any memory layout by internally managing multiple [`OpaquePool`] instances.
//!
//! It offers stable memory addresses and efficient typed insertion with automatic value dropping.
//!
//! # Type-agnostic memory management
//!
//! Unlike [`OpaquePool`] which requires a specific layout to be defined at creation time, 
//! [`BlindPool`] can accept any type and will automatically create and manage the appropriate
//! internal pools as needed. Each distinct memory layout gets its own internal [`OpaquePool`].
//!
//! The pool itself does not hold or create any `&` shared or `&mut` exclusive references to its
//! contents, allowing the caller to decide who and when can obtain a reference to the inserted
//! values. The caller is responsible for ensuring that Rust aliasing rules are respected.
//!
//! # Features
//!
//! - **Layout-agnostic memory management**: Accepts any type and manages layouts automatically.
//! - **Stable addresses**: Memory addresses remain valid until explicitly removed.
//! - **Automatic dropping**: Values are properly dropped when removed from the pool.
//! - **Dynamic growth**: Pool capacity grows automatically as needed.
//! - **Efficient allocation**: Uses high density slabs to minimize allocation overhead.
//! - **Stable Rust**: No unstable Rust features required.
//! - **Leak detection**: Pool panics on drop if values are still present.
//!
//! # Example
//!
//! ```rust
//! use blind_pool::BlindPool;
//!
//! // Create a blind pool that can store any type.
//! let mut pool = BlindPool::new();
//!
//! // Insert values of different types into the same pool.
//! let pooled_u64 = pool.insert(42_u64);
//! let pooled_string = pool.insert("hello".to_string());
//! let pooled_f32 = pool.insert(3.14_f32);
//!
//! // Read data back from the pooled items.
//! let value_u64 = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     pooled_u64.ptr().read()
//! };
//!
//! let value_string = unsafe {
//!     // SAFETY: The pointer is valid and the value was just inserted.
//!     pooled_string.ptr().read()
//! };
//!
//! assert_eq!(value_u64, 42);
//! assert_eq!(value_string, "hello");
//!
//! // Remove values from the pool. The values are automatically dropped.
//! pool.remove(pooled_u64);
//! pool.remove(pooled_string);
//! pool.remove(pooled_f32);
//!
//! assert!(pool.is_empty());
//! ```

mod builder;
mod pool;

pub use builder::*;
// Re-export DropPolicy from opaque_pool
pub use opaque_pool::DropPolicy;
pub use pool::*;