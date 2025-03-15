// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

//! Mechanisms for creating families of linked objects that can collaborate across threads
//! while being internally single-threaded.
//!
//! The problem this crate solves is that while writing highly efficient lock-free thread-local
//! code can yield great performance, it comes with serious drawbacks in terms of usability and
//! developer experience.
//!
//! This crate bridges the gap by providing patterns and mechanisms that facilitate thread-local
//! behavior while presenting a simple and reasonably ergonomic API to user code:
//!
//! * Internally, a linked object can take advantage of lock-free thread-isolated logic for **high
//!   performance and efficiency** because it operates as a multithreaded family of single-threaded
//!   objects, each of which implements local behavior on a single thread.
//! * Externally, the linked object looks and acts very much like a regular Rust object and can
//!   operate on multiple threads, providing **a reasonably simple API with minimal extra
//!   complexity**.
//!
#![ doc=mermaid!( "../doc/linked.mermaid") ]
//!
//!
//! The patterns and mechanisms provided by this crate are designed to make it easy to create such
//! object families and to provide primitives that allow these object families to be used without
//! the user code having to understand how the objects are wired up inside or keeping track of which
//! instance is meant to be used where and on which thread.
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # What is a linked object?
//!
//! Linked objects are types decorated with `#[linked::object]` whose instances:
//!
//! 1. are each local to a single thread (i.e. [`!Send`][4]); (see also, [linked objects on multiple
//!    threads][multiple-threads])
//! 1. and are internally connected to other instances from the same family (note that multiple
//!    families of the same type may exist, each instance belongs to exactly one);
//! 1. and share some state between instances in the same family, e.g. via messaging or synchronized
//!    storage;
//! 1. and perform all collaboration between instances in the same family without involvement of
//!    user code (i.e. there is no `Arc` or `Mutex` that the user needs to create/operate).
//!
//! In most cases, as long as logic is thread-local, user code can treat linked objects like any
//! other Rust structs. The mechanisms only have an effect when [instances on multiple threads need
//! to collaborate][multiple-threads].
//!
//! Note that despite instances of linked objects being designed for thread-local use, there may
//! still exist multiple instances per thread in the same family. You can explicitly opt-in to
//! "one per thread" behavior via the [`linked::PerThread<T>`][3] wrapper.
//!
//! # What is a family of linked objects?
//!
//! A family of linked objects is the unit of collaboration between instances. Each instance in a
//! family can communicate with all other instances in the same family through shared state or other
//! synchronization mechanisms. They act as a single distributed object, exhibiting thread-local
//! behavior by default and internally triggering global behavior as needed.
//!
//! Instances are defined as belonging to the same family if they:
//!
//! - are created via cloning;
//! - or are created by obtaining a thread-safe [Handle] from another family member, which is
//!   thereafter converted to a new instance (potentially on a different thread);
//! - or are obtained from the same static variable in a [`linked::instance_per_access!`][1]
//!   or [`linked::instance_per_thread!`][2] macro block;
//! - or are created from the same [`linked::PerThread<T>`][3] or one of its clones.
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
//! * Implementing the collaboration between instances does not require anything (e.g. a `Mutex`
//!   or `mpsc::channel`) from user code.
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
//!             // Capture `shared_value` to reuse it for all instances in the family.
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
//! Note: because this is a contrived example, this type is not very useful as it does not have
//! any high-efficiency thread-local logic that would benefit from the linked object pattern. See
//! the [Implementing local behavior][local-behavior] section.
//!
//! The implementation steps to apply the pattern to a struct are:
//!
//! * Apply [`#[linked::object]`][crate::object] on the struct. This will automatically
//!   derive the `linked::Object` and `Clone` traits and implement various other behind-the-scenes
//!   mechanisms required for the linked object pattern to operate.
//! * In the constructor, call [`linked::new!`][crate::new] to create the first instance.
//!
//! [`linked::new!`][crate::new] is a wrapper around a `Self` struct-expression. What makes
//! it special is that **this struct-expression will be called for every instance that is ever
//! created in the same family of linked objects**. This expression captures the state of the
//! constructor (e.g. in the above example, it captures `shared_value`). Use the captured
//! state to set up any shared connections between instances in the same family (e.g. by sharing
//! an `Arc` or connecting message channels).
//!
//! The captured values must be thread-safe (`Send` + `Sync` + `'static`), while the `Thing`
//! struct itself does not need to be thread-safe. In fact, the linked object pattern forces
//! it to be `!Send` and `!Sync` to avoid accidental multithreaded use of a single instance.
//! See the next chapter to understand how to implement multithreaded logic.
//!
//! # Linked objects on multiple threads
//! [multiple-threads]: #multiple-threads
//!
//! Each instance of a linked object is single-threaded (enforced at compile time via `!Send`). This
//! raises an obvious question: how can I create different instances on different threads if the
//! instances cannot be sent between threads?
//!
//! There are three mechanisms for this:
//!
//! * Use a static variable in a [`linked::instance_per_access!`][1] or
//!   [`linked::instance_per_thread!`][2] block, with the former creating a new linked instance
//!   on each access and the latter reusing per-thread instances. All instances resolved via such
//!   static variables are linked to the same family.
//! * Use a [`linked::PerThread<T>`][3] wrapper to create a thread-local instance of `T`. You can
//!   freely send the `PerThread<T>` or its clones between threads, unpacking it into a thread-local
//!   linked instance once the `PerThread<T>` arrives on the destination thread.
//! * Use a [`Handle`] to transfer an instance between threads. A handle is a thread-safe reference
//!   to a linked object family, from which instances belonging to that family can be created. This
//!   is the low-level primitive used by all the other mechanisms internally.
//!
//! You can think of a `PerThread<T>` as a special-case `Handle<T>` that always returns the same
//! instance on the same thread, unlike `Handle<T>` which can be used to create any number of
//! separate instances on any thread. The purpose of the static variables and `PerThread<T>` is
//! to minimize the bookkeeping required in user code to manage the linked object instances.
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
//! Example of using a [`PerThread<T>`][3] to create thread-local instances on each thread:
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
//! use linked::PerThread;
//! use std::thread;
//!
//! let thing_per_thread = PerThread::new(Thing::new("hello".to_string()));
//!
//! // Obtain a local instance on demand.
//! let thing = thing_per_thread.local();
//! assert_eq!(thing.value(), "hello");
//!
//! thing.set_value("world".to_string());
//!
//! // We move the `thing_per_thread` to another thread (you can also just clone it).
//! thread::spawn(move || {
//!     let thing = thing_per_thread.local();
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
//! # Implementing local behavior
//! [local-behavior]: #local-behavior
//!
//! The linked object pattern does not change the fact that synchronized state is expensive.
//! Whenever possible, linked objects should operate on local state - the entire purpose of this
//! pattern is to put local behavior front and center and make any sharing require explicit intent.
//!
//! Let's extend the above example type with a local counter that counts the number of times
//! the value has been modified via the current instance. This is local behavior that does not
//! require any synchronization with other instances.
//!
//! ```rust
//! use std::sync::{Arc, Mutex};
//!
//! #[linked::object]
//! pub struct Thing {
//!     // Shared state - synchronized with other instances in the family.
//!     value: Arc<Mutex<String>>,
//!
//!     // Local state - not synchronized with other instances in the family.
//!     update_count: usize,
//! }
//!
//! impl Thing {
//!     pub fn new(initial_value: String) -> Self {
//!         let shared_value = Arc::new(Mutex::new(initial_value));
//!
//!         linked::new!(Self {
//!             // Capture `shared_value` to reuse it for all instances in the family.
//!             value: Arc::clone(&shared_value),
//!
//!             // Local state is simply initialized to 0 for every instance.
//!             update_count: 0,
//!         })
//!     }
//!
//!     pub fn value(&self) -> String {
//!         self.value.lock().unwrap().clone()
//!     }
//!
//!     pub fn set_value(&mut self, value: String) {
//!         *self.value.lock().unwrap() = value;
//!          self.update_count += 1;
//!     }
//!
//!     pub fn update_count(&self) -> usize {
//!         self.update_count
//!     }
//! }
//! ```
//!
//! Local behavior consists of simply operating on regular non-synchronized fields of the struct.
//!
//! However, note that we had to modify `set_value()` to receive `&mut self` instead of the
//! previous `&self`. This is because we now need to modify a field of the object, so we need
//! an exclusive `&mut` reference to the instance.
//!
//! `&mut self` is not a universally applicable technique - if you are using a linked object in
//! per-thread mode then all access must be via `&self` shared references because there can be
//! multiple references simultaneously alive per thread. This means there can be no `&mut self`
//! and we need to use interior mutability (e.g. [`Cell`][6], [`RefCell`][7], etc.) to support
//! local behavior together with per-thread instantiation.
//!
//! Attempting to use the above example type in a per-thread context will simply mean that the
//! `set_value()` method cannot be called because there is no way to create a `&mut self` reference.
//!
//! Example of the same type using [`Cell`][6] to support local behavior
//! without `&mut self`:
//!
//! ```rust
//! use std::cell::Cell;
//! use std::sync::{Arc, Mutex};
//!
//! #[linked::object]
//! pub struct Thing {
//!     // Shared state - synchronized with other instances in the family.
//!     value: Arc<Mutex<String>>,
//!
//!     // Local state - not synchronized with other instances in the family.
//!     update_count: Cell<usize>,
//! }
//!
//! impl Thing {
//!     pub fn new(initial_value: String) -> Self {
//!         let shared_value = Arc::new(Mutex::new(initial_value));
//!
//!         linked::new!(Self {
//!             // Capture `shared_value` to reuse it for all instances in the family.
//!             value: Arc::clone(&shared_value),
//!
//!             // Local state is simply initialized to 0 for every instance.
//!             update_count: Cell::new(0),
//!         })
//!     }
//!
//!     pub fn value(&self) -> String {
//!         self.value.lock().unwrap().clone()
//!     }
//!
//!     pub fn set_value(&self, value: String) {
//!         *self.value.lock().unwrap() = value;
//!          self.update_count.set(self.update_count.get() + 1);
//!     }
//!
//!     pub fn update_count(&self) -> usize {
//!         self.update_count.get()
//!     }
//! }
//! ```
//!
//! # Using linked objects via abstractions
//!
//! You may find yourself in a situation where you need to use a linked object type `T` through
//! a trait object of a trait `Xyz`, where `T: Xyz`. That is, you may want to use your `T` as a
//! `dyn Xyz`. This is a common pattern in Rust but with the linked objects pattern there is
//! a choice you must make:
//!
//! * If the linked objects are **always** to be accessed via trait objects (`dyn Xyz`), wrap
//!   the `dyn Xyz` instances in [`linked::Box`], returning such a box already in the constructor.
//! * If the linked objects are **sometimes** to be accessed via trait objects, you can on-demand
//!   wrap them into a [`std::boxed::Box<dyn Xyz>`][5].
//!
//! The difference is that [`linked::Box`] preserves the linked object functionality even for the
//! `dyn Xyz` form - you can clone the box, obtain a [`Handle<linked::Box<dyn Xyz>>`][Handle] to
//! extend the object family to another thread and store such a box in a static variable in a
//! [`linked::instance_per_access!`][1] or [`linked::instance_per_thread!`][2] block or a
//! [`PerThread<T>`][3] for automatic instance management.
//!
//! In contrast, when you use a [`std::boxed::Box<dyn Xyz>`][5], you lose the linked
//! object functionality (but only for the instance that you put in the box). Internally, the boxed
//! instance keeps working as it always did but you cannot use the linked object API on it, such
//! as obtaining a handle.
//!
//! Example of using a linked object via a trait object using [`linked::Box`], for scenarios
//! where the linked object is always accessed via a trait object:
//!
//! ```rust
//! # trait ConfigSource {}
//! # impl ConfigSource for XmlConfig {}
//! // If using linked::Box, do not put `#[linked::object]` on the struct.
//! // The linked::Box itself is the linked object and our struct is only its contents.
//! struct XmlConfig {
//!     config: String
//! }
//!
//! impl XmlConfig {
//!     pub fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
//!         // Constructing instances works logically the same as for regular linked objects.
//!         //
//!         // The only differences are:
//!         // 1. We use `linked::new_box!` instead of `linked::new!`
//!         // 2. There is an additional parameter to the macro to name the trait object type.
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
//! Example of using a linked object via a trait object using [`std::boxed::Box<dyn Xyz>`][5],
//! for scenarios where the linked object is only sometimes accessed via a trait object:
//!
//! ```rust
//! # trait ConfigSource {}
//! # impl ConfigSource for XmlConfig {}
//! #[linked::object]
//! struct XmlConfig {
//!     config: String
//! }
//!
//! impl XmlConfig {
//!     // XmlConfig itself is a regular linked object, nothing special about it.
//!     pub fn new() -> XmlConfig {
//!         linked::new!(
//!             Self {
//!                 config: "xml".to_string(),
//!             }
//!         )
//!     }
//!
//!     // When the caller wants a `dyn ConfigSource`, we can convert this specific instance into
//!     // one. The trait object loses its linked objects API surface (though remains part of the
//!     // family).
//!     pub fn into_config_source(self) -> Box<dyn ConfigSource> {
//!         Box::new(self)
//!     }
//! }
//! ```
//!
//! # Additional examples
//!
//! See `examples/linked_*.rs` for more examples of using linked objects in different scenarios.
//!
//! [1]: crate::instance_per_access
//! [2]: crate::instance_per_thread
//! [3]: crate::PerThread
//! [4]: https://doc.rust-lang.org/nomicon/send-and-sync.html
//! [5]: std::boxed::Box
//! [6]: std::cell::Cell
//! [7]: std::cell::RefCell

use simple_mermaid::mermaid;

#[doc(hidden)]
pub mod __private;

mod r#box;
mod constants;
mod handle;
mod object;
mod per_access_static;
mod per_thread;
mod per_thread_static;

pub use r#box::*;
pub(crate) use constants::*;
pub use handle::*;
pub use object::*;
pub use per_access_static::*;
pub use per_thread::*;
pub use per_thread_static::*;

mod macros;

/// Marks a struct as implementing the [linked object pattern][crate].
///
/// See crate-level documentation for a high-level guide.
///
/// # Usage
///
/// Apply the attribute on a struct block. Then, in constructors, use the
/// [`linked::new!`][crate::new] macro to create an instance of the struct.
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
/// 1. Generates the necessary wiring to support calling `linked::new!` in constructors to create
///    instances. This macro disables regular `Self {}` struct-expressions and requires the use
///    of `linked::new!`.
/// 2. Implements `Clone` for the struct. All linked objects can be cloned to create new
///    instances linked to the same family.
/// 3. Implements the trait [`linked::Object`] for the struct, enabling standard linked object
///    pattern mechanisms such as calling `.handle()` on instances to obtain [`Handle`]s.
/// 4. Implements `From<linked::Handle<T>>` for the struct. This allows converting a [`Handle`]
///    into an instance of the linked object using `.into()`.
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
