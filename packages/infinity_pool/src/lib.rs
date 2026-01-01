#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Infinity pool: object pools with trait object support and multiple access models.
//!
//! This package provides several types of object pools designed for different use cases,
//! from basic pooling to advanced memory layouts with custom drop policies.
//!
//! # Motivating scenario
//!
//! The primary characteristics of the target scenario are:
//!
//! * You need to create and destroy many objects on a regular basis.
//! * These objects need to be pinned.
//! * You want better performance than you get from `Box::pin()`.
//! * Optionally, you may also want to store references to the objects in the form of
//!   trait object references (`&dyn Foo`), perhaps because you cannot name the type
//!   due to it being inferred.
//! * Optionally, you may want to use reference counting to manage object lifetimes.
//!
//! It would be fair to say that this package essentially provides faster alternatives
//! to `Box::pin()`, `Arc::pin()` and `Rc::pin()`.
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
//! In addition to the above characteristics,
//! [the objects in all the pools can be accessed as trait objects][casting],
//! thereby ensuring you do not need to name any types even at point of use, and can pass
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
//! | Pinned Pool               | [`PinnedPool<T>`] | [`LocalPinnedPool<T>`] | [`RawPinnedPool<T>`] |
//! | Opaque Pool               | [`OpaquePool`] | [`LocalOpaquePool`] | [`RawOpaquePool`] |
//! | Blind Pool                | [`BlindPool`]  | [`LocalBlindPool`]  | [`RawBlindPool`]  |
//!
//! # Performance
//!
//! On an arbitrary x64 machine running Windows, the pools provided by this package offer better
//! performance than the equivalent standard library primitives (`Box::pin()`, `Arc::pin()`, `Rc::pin()`).
//!
//! <img src="https://media.githubusercontent.com/media/folo-rs/folo/refs/heads/main/packages/infinity_pool/benchmark_results.png">
//!
//! # Examples
//!
//! ## Pinned pool (thread-safe, single type)
//!
//! ```
//! use infinity_pool::PinnedPool;
//!
//! let pool = PinnedPool::<String>::new();
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
//!     let pool = OpaquePool::with_layout_of::<T>();
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
//! let pool = BlindPool::new();
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
//!
//! use infinity_pool::{BlindPool, BlindPooledMut, define_pooled_dyn_cast};
//!
//! // Enable casting to Display trait objects
//! define_pooled_dyn_cast!(Display);
//!
//! // Function that accepts trait object handles directly
//! fn process_displayable(handle: BlindPooledMut<dyn Display>) {
//!     println!("Processing: {}", &*handle);
//! }
//!
//! let pool = BlindPool::new();
//! let string_handle = pool.insert("Hello, world!".to_string());
//! let number_handle = pool.insert(42i32);
//!
//! // Cast to trait objects and pass to function
//! process_displayable(string_handle.cast_display());
//! process_displayable(number_handle.cast_display());
//! ```
//!
//! [casting]: define_pooled_dyn_cast

mod blind;
mod builders;
mod cast;
mod drop_policy;
mod handles;
mod opaque;
mod pinned;
#[cfg(test)]
mod thread_safety_types;

pub use blind::*;
pub use builders::*;
pub use drop_policy::*;
pub use handles::*;
pub use opaque::*;
// Re-export so we can use it without the consumer needing a reference.
#[doc(hidden)]
pub use pastey::paste as __private_paste;
pub use pinned::*;
#[cfg(test)]
pub(crate) use thread_safety_types::*;
