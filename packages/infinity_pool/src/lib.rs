//! Infinity Pool: Advanced object pool implementations with flexible memory management.
//!
//! This crate provides several types of object pools designed for different use cases,
//! from basic pooling to advanced memory layouts with custom drop policies.
//!
//! # Pool types
//!
//! * Pinned pool - the most basic object pool, resembling a `Vec<T>` that guarantees all its
//!   elements are pinned in memory.
//! * Opaque pool - its type signature does not name the type of the object, allowing you to
//!   maintain a pool of objects with an unnameable type (e.g. the type behind an `impl Future`).
//! * Blind pool - accepts any type of object, including multiple different types in the same pool,
//!   without requiring any of the types to be named (e.g. many different `impl Future` types).
//!
//! In addition to the above characteristics, the objects in all the pools can be accessed as trait
//! objects, thereby ensuring you do not need to name any types even at point of use, and can pass
//! objects around between APIs that extend beyond `impl Future` style type inference.
//!   
//! # Access models
//!
//! Every pool offers three access models:
//!
//! * The default access model is thread-safe and uses reference counted handles (`Arc` style)
//!   to control inserted object lifetime.
//! * A single-threaded access model is available as an alternative, offering better performance
//!   when you do not need thread safety. It also uses reference counted handles (`Rc` style)
//!   to control inserted object lifetime.
//! * For advanced scenarios, a "raw" access model is available, which does not use
//!   reference counting and requires the user to manage object lifetimes manually using
//!   unsafe code.
//!
//! The raw access model offers maximum performance and efficiency but at the cost of requiring
//! unsafe code to manage lifetimes and access the objects.
//!
//! # Pool choice matrix
//!
//! Select the appropriate type based on the pool characteristics and access model you need:
//!
//! | Pool Type / Access Model  | `Arc`-like | `Rc`-like | Raw |
//! |---------------------------|------------|-----------|-----|
//! | Pinned Pool               | [`PinnedPool`] | [`LocalPinnedPool`] | [`RawPinnedPool`] |
//! | Opaque Pool               | [`OpaquePool`] | [`LocalOpaquePool`] | [`RawOpaquePool`] |
//! | Blind Pool                | [`BlindPool`]  | [`LocalBlindPool`]  | [`RawBlindPool`]  |
//!
//! # Examples
//!
//! ## Pinned pool (thread-safe, single type)
//!
//! ```
//! use infinity_pool::PinnedPool;
//!
//! let mut pool = PinnedPool::<String>::new();
//! let handle = pool.insert("Hello, world!".to_string());
//! assert_eq!(&*handle, "Hello, world!");
//! ```
//!
//! ## Opaque pool (thread-safe, unnamed type)
//!
//! ```
//! use infinity_pool::OpaquePool;
//!
//! fn work_with_displayable<T: std::fmt::Display + Send + 'static>(value: T) {
//!     let mut pool = OpaquePool::with_layout_of::<T>();
//!     let handle = pool.insert(value);
//!     println!("Stored: {}", &*handle);
//! }
//!
//! work_with_displayable("Hello, world!");
//! work_with_displayable(42);
//! ```
//!
//! ## Blind pool (thread-safe, multiple unnamed types)
//!
//! ```
//! use infinity_pool::BlindPool;
//!
//! let mut pool = BlindPool::new();
//! let string_handle = pool.insert("Hello, world!".to_string());
//! let number_handle = pool.insert(42u32);
//! assert_eq!(&*string_handle, "Hello, world!");
//! assert_eq!(*number_handle, 42);
//! ```
//!
//! ## Trait object usage with [`BlindPool`]
//!
//! ```
//! use std::fmt::Display;
//! use infinity_pool::{BlindPool, Pooled, define_pooled_dyn_cast};
//!
//! // Enable casting to Display trait objects
//! define_pooled_dyn_cast!(Display);
//!
//! // Function that accepts trait object handles directly
//! fn process_displayable(handle: Pooled<dyn Display>) {
//!     println!("Processing: {}", &*handle);
//! }
//!
//! let mut pool = BlindPool::new();
//! let string_handle = pool.insert("Hello, world!".to_string());
//! let number_handle = pool.insert(42i32);
//!
//! // Cast to trait objects and pass to function
//! process_displayable(string_handle.cast_display());
//! process_displayable(number_handle.cast_display());
//! ```

mod blind;
mod builders;
mod cast;
mod constants;
mod drop_policy;
mod handles;
mod opaque;
mod pinned;

pub use blind::*;
pub use builders::*;
pub(crate) use constants::*;
pub use drop_policy::*;
pub use handles::*;
pub use opaque::*;
// Re-export so we can use it without the consumer needing a reference.
#[doc(hidden)]
pub use pastey::paste as __private_paste;
pub use pinned::*;
