// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

//! Mechanisms for creating families of linked objects that can collaborate across threads while being
//! internally single-threaded.
//!
//! The linked object pattern ensures that cross-thread state sharing is always explicit, as well as
//! cross-thread transfer of linked object instances, facilitated by the mechanisms in this crate. Each
//! individual instance of a linked object and the mechanisms for obtaining new instances are
//! structured in a manner that helps avoid accidental or implicit shared state, by making each instance
//! thread-local while the entire family can act together to provide a multithreaded API to user code.
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # Definitions
//!
//! Linked objects are types whose instances:
//!
//! 1. are each local to a single thread (i.e. `!Send`);
//! 1. and are internally connected to other instances from the same family;
//! 1. and share some thread-safe state via messaging or synchronized state;
//! 1. and perform all collaboration between instances without involvement of user code (i.e. there is
//!    no `Arc` or `Mutex` that the user needs to create).
//!
//! Note that despite instances of linked objects being thread-local (`!Send`), there may still be
//! multiple instances per thread.
//!
//! Instances belong to the same family if they:
//!
//! - are created via cloning;
//! - or are created by obtaining a thread-safe [Handle] and converting it to a new instance;
//! - or are obtained from the same static variable in a [`linked::instance_per_access!`][1]
//!   or [`linked::instance_per_thread!`][2] macro block.
//!
//! [1]: crate::instance_per_access
//! [2]: crate::instance_per_thread
//!
//! # Using and defining linked objects
//!
//! A very basic and contrived example is a `Thing` that shares a `value` between all its instances.
//!
//! This object can generally be used like any other Rust type. All linked objects support cloning,
//! since that is one of the primary mechanisms for creating additional linked instances.
//!
//! ```rust
//! # use std::sync::{Arc, Mutex};
//! # #[linked::object]
//! # struct Thing {
//! #     value: Arc<Mutex<String>>,
//! # }
//! # impl Thing {
//! #     pub fn new(initial_value: String) -> Self {
//! #         let shared_value = Arc::new(Mutex::new(initial_value));
//! #         linked::new!(Self {
//! #             value: shared_value.clone(),
//! #         })
//! #     }
//! #     pub fn value(&self) -> String {
//! #         self.value.lock().unwrap().clone()
//! #     }
//! #     pub fn set_value(&self, value: String) {
//! #         *self.value.lock().unwrap() = value;
//! #     }
//! # }
//! let thing1 = Thing::new("hello".to_string());
//! let thing2 = thing1.clone();
//! assert_eq!(thing1.value(), "hello");
//! assert_eq!(thing2.value(), "hello");
//!
//! thing1.set_value("world".to_string());
//! assert_eq!(thing1.value(), "world");
//! assert_eq!(thing2.value(), "world");
//! ```
//!
//! We can compare this example to the linked object definition above:
//!
//! * The relation between instances is established via cloning.
//! * The `value` is shared.
//! * Implementing the collaboration does not require anything (e.g. a `Mutex`) from user code.
//!
//! The implementation of this type is the following:
//!
//! ```rust
//! use std::sync::{Arc, Mutex};
//!
//! #[linked::object]
//! pub struct Thing {
//!     value: Arc<Mutex<String>>,
//! }
//!
//! impl Thing {
//!     pub fn new(initial_value: String) -> Self {
//!         let shared_value = Arc::new(Mutex::new(initial_value));
//!
//!         linked::new!(Self {
//!             value: Arc::clone(&shared_value),
//!         })
//!     }
//!
//!     pub fn value(&self) -> String {
//!         self.value.lock().unwrap().clone()
//!     }
//!
//!     pub fn set_value(&self, value: String) {
//!         *self.value.lock().unwrap() = value;
//!     }
//! }
//! ```
//!
//! The implementation steps to apply the pattern to a struct are:
//!
//! * Apply [`#[linked::object]`][crate::object] on the struct. This will automatically
//!   derive the `linked::Object` and `Clone` traits and implement various other behind-the-scenes
//!   mechanisms required for the linked object pattern to operate.
//! * In the constructor, call [`linked::new!`][crate::new] to create the first instance.
//!
//! [`linked::new!`][crate::new] is a wrapper around a `Self` struct-expression. What makes
//! it special is that **it will be called for every instance that is ever created in the same family
//! of linked objects**. This expression captures the state of the constructor (e.g. in the above
//! example, it captures `shared_value`). Use the captured state to set up any shared connections
//! between instances (e.g. by sharing an `Arc` or connecting message channels).
//!
//! The captured values must be thread-safe (`Send` + `Sync` + `'static`), while the `Thing` struct
//! itself does not need to be thread-safe. In fact, the linked object pattern forces it to be `!Send`
//! and `!Sync` to avoid accidental multithreading. See the next chapter to understand how to deal with
//! multithreaded logic.
//!
//! # Linked objects on multiple threads
//!
//! Each instance of a linked object is single-threaded (enforced at compile time). To create a
//! related instance on a different thread, you must either use a static variable inside a
//! [`linked::instance_per_access!`][1] or [`linked::instance_per_thread!`][2] block or
//! obtain a [Handle] that you can transfer to another thread and use to obtain a new instance there.
//! Linked object handles are thread-safe.
//!
//! Example of using a static variable to connect instances on different threads:
//!
//! ```rust
//! # use std::sync::{Arc, Mutex};
//! # #[linked::object]
//! # struct Thing {
//! #     value: Arc<Mutex<String>>,
//! # }
//! # impl Thing {
//! #     pub fn new(initial_value: String) -> Self {
//! #         let shared_value = Arc::new(Mutex::new(initial_value));
//! #         linked::new!(Self {
//! #             value: shared_value.clone(),
//! #         })
//! #     }
//! #     pub fn value(&self) -> String {
//! #         self.value.lock().unwrap().clone()
//! #     }
//! #     pub fn set_value(&self, value: String) {
//! #         *self.value.lock().unwrap() = value;
//! #     }
//! # }
//! use std::thread;
//!
//! linked::instance_per_access!(static THE_THING: Thing = Thing::new("hello".to_string()));
//!
//! let thing = THE_THING.get();
//! assert_eq!(thing.value(), "hello");
//!
//! thing.set_value("world".to_string());
//!
//! thread::spawn(|| {
//!     let thing = THE_THING.get();
//!     assert_eq!(thing.value(), "world");
//! }).join().unwrap();
//! ```
//!
//! Example of using a [Handle] to transfer an instance to another thread:
//!
//! ```rust
//! # use std::sync::{Arc, Mutex};
//! # #[linked::object]
//! # struct Thing {
//! #     value: Arc<Mutex<String>>,
//! # }
//! # impl Thing {
//! #     pub fn new(initial_value: String) -> Self {
//! #         let shared_value = Arc::new(Mutex::new(initial_value));
//! #         linked::new!(Self {
//! #             value: shared_value.clone(),
//! #         })
//! #     }
//! #     pub fn value(&self) -> String {
//! #         self.value.lock().unwrap().clone()
//! #     }
//! #     pub fn set_value(&self, value: String) {
//! #         *self.value.lock().unwrap() = value;
//! #     }
//! # }
//! use linked::Object; // This brings .handle() into scope.
//! use std::thread;
//!
//! let thing = Thing::new("hello".to_string());
//! assert_eq!(thing.value(), "hello");
//!
//! thing.set_value("world".to_string());
//!
//! let thing_handle = thing.handle();
//!
//! thread::spawn(|| {
//!     let thing: Thing = thing_handle.into();
//!     assert_eq!(thing.value(), "world");
//! }).join().unwrap();
//! ```
//!
//! # Using linked objects via abstractions
//!
//! You may find yourself in a situation where you need to use a linked object type `T` through
//! a trait object of a trait `Xyz` that `T` implements, as `dyn Xyz`. This is a common pattern
//! in Rust but with the linked objects pattern there is a choice you must make:
//!
//! * If the linked objects are **always** to be accessed via trait objects, wrap the instances in
//!   [`linked::Box`], returning such a box already in the constructor.
//! * If the linked objects are **sometimes** to be accessed via trait objects, you can on-demand
//!   wrap them into a [`Box<dyn Xyz>`][std::boxed::Box].
//!
//! The difference is that [`linked::Box`] preserves the linked object functionality - you can
//! clone the box, obtain a [`Handle<linked::Box<dyn Xyz>>`][Handle] to transfer the box to another
//! thread and store such a box in a static variable in a [`linked::instance_per_access!`][1] or
//! [`linked::instance_per_thread!`][2] block. However, when you use a
//! [`Box<dyn Xyz>`][std::boxed::Box], you lose the linked object functionality (but only for the
//! instance that you put in the box).
//!
//! ```rust
//! # trait ConfigSource {}
//! # struct XmlConfig { config: String }
//! # impl ConfigSource for XmlConfig {}
//! impl XmlConfig {
//!     pub fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
//!         linked::new_box!(
//!             dyn ConfigSource,
//!             Self {
//!                 config: "xml".to_string(),
//!             }
//!         )
//!     }
//! }
//! ```
//!
//! # Additional examples
//!
//! See `examples/linked_*.rs` for more examples of using linked objects in different scenarios.

#[doc(hidden)]
pub mod __private;

mod r#box;
mod handle;
mod object;
mod per_access;
mod per_thread;

pub use r#box::*;
pub use handle::*;
pub use object::*;
pub use per_access::*;
pub use per_thread::*;

mod macros;

/// Marks a struct as implementing the [linked object pattern][crate].
///
/// # Usage
///
/// Apply attribute on a struct block with named fields.
///
/// # Example
///
/// ```
/// #[linked::object]
/// pub struct TokenCache {
///     some_value: usize,
/// }
///
/// impl TokenCache {
///     pub fn new(some_value: usize) -> Self {
///         linked::new!(Self {
///             some_value,
///         })
///     }
/// }
/// ```
///
/// # Effects
///
/// Applying this macro has the following effects:
///
/// 1. Generates the necessary wiring to support calling `linked::new!` in constructors.
/// 2. Implements `Clone` for the struct. All linked objects can be cloned to create new
///    instances linked to the same family.
/// 3. Implements the trait `linked::Linked` for the struct, enabling standard linked object
///    pattern mechanisms such as calling `.handle()` on instances.
/// 4. Implements `From<linked::Handle<T>>` for the struct. This allows creating a new
///    linked instance from a handle previously obtained from `.handle()`.
/// 5. Removes `Send` and `Sync` traits implementation for the struct. Linked objects are single-
///    threaded and require explicit steps to transfer instances across threads (e.g. via handles).
///
/// # Constraints
///
/// Only structs defined in the named fields form are supported (no tuple structs).
pub use linked_macros::__macro_linked_object as object;

// This is so procedural macros can produce code which refers to
// ::linked::* which will work also in the current crate.
#[doc(hidden)]
extern crate self as linked;
