// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Mechanisms for creating families of linked objects that can collaborate across threads,
//! with each instance only used from a single thread.
//!
//! The problem this package solves is that while writing highly efficient lock-free thread-local
//! code can yield great performance, it comes with serious drawbacks in terms of usability and
//! developer experience.
//!
//! This package bridges the gap by providing patterns and mechanisms that facilitate thread-local
//! behavior while presenting a simple and reasonably ergonomic API to user code:
//!
//! * Internally, a linked object can take advantage of lock-free thread-isolated logic for **high
//!   performance and efficiency** because it operates as a multithreaded family of thread-isolated
//!   objects, each of which implements local behavior on a single thread.
//! * Externally, the linked object family can look and act very much like a single Rust object and
//!   can hide the fact that there is collaboration happening on multiple threads,
//!   providing **a reasonably simple API with minimal extra complexity** for both the author
//!   and the user of a type.
#![ doc=mermaid!( "../doc/linked.mermaid") ]
//!
//! The patterns and mechanisms provided by this package are designed to make it easy to create linked
//! object families and to provide primitives that allow these object families to be used without
//! the user code having to understand how the objects are wired up inside or keeping track of which
//! instance is meant to be used on which thread.
//!
//! This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
//! high-performance hardware-aware programming in Rust.
//!
//! # What is a linked object?
//!
//! Linked objects defined by types decorated with `#[linked::object]` whose instances:
//!
//! 1. take advantage of thread-specific state by being functionally single-threaded
//!    (see also, [linked objects on multiple threads][multiple-threads]);
//! 1. and are internally connected to other instances from the same family;
//! 1. and share some state between instances in the same family, e.g. via messaging or synchronized
//!    storage;
//! 1. and perform all collaboration between instances in the same family without involvement of
//!    user code (i.e. there is no `Arc` or `Mutex` that the user needs to create/operate).
//!
//! In most cases, as long as user code executes thread-local logic, user code can treat linked
//! objects like any other Rust structs. The mechanisms only have an effect when
//! [instances on multiple threads need to collaborate][multiple-threads].
//!
//! Despite instances of linked objects being designed for thread-local use, there may
//! still exist multiple instances per thread in the same family because this is meaningful for
//! some types. For example, think of messaging channels - multiple receivers should be independent,
//! even on the same thread, even if part of the same channel.
//!
//! The primary mechanisms this package provides to create instances of linked objects are:
//!
//! * [`linked::instances!` macro][1];
//! * [`linked::Family<T>`][11]
//!
//! The above will create instances on demand, including creating multiple instances on the same
//! thread if asked to.
//!
//! You can explicitly opt-in to "one per thread" behavior via these additional mechanisms:
//!
//! * [`linked::thread_local_rc!` macro][2]
//! * [`linked::thread_local_arc!` macro][8] (if `T: Sync`)
//! * [`linked::InstancePerThread<T>`][3].
//! * [`linked::InstancePerThreadSync<T>`][9] (if `T: Sync`)
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
//! - or are obtained from the same static variable in a [`linked::instances!`][1],
//!   [`linked::thread_local_rc!`][2] or [`linked::thread_local_arc!`][8] macro block;
//! - or are created from the same [`linked::InstancePerThread<T>`][3] or one of its clones;
//! - or are created from the same [`linked::InstancePerThreadSync<T>`][9] or one of its clones;
//! - or are created directly from a [`linked::Family<T>`][11].
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
//! // These instances are part of the same family due to cloning.
//! let thing1 = Thing::new("hello".to_string());
//! let thing2 = thing1.clone();
//!
//! assert_eq!(thing1.value(), "hello");
//! assert_eq!(thing2.value(), "hello");
//!
//! thing1.set_value("world".to_string());
//!
//! // The value is shared between instances in the same family.
//! assert_eq!(thing1.value(), "world");
//! assert_eq!(thing2.value(), "world");
//! ```
//!
//! We can compare this example to the linked object definition above:
//!
//! * The relation between instances is established via cloning.
//! * The `value` is shared.
//! * Implementing the collaboration between instances does not require anything from user code
//!   (e.g. there is no `Mutex` or `mpsc::channel` that had to be written here).
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
//! As this is a contrived example, this type is not very useful because it does not have
//! any high-efficiency thread-local logic that would benefit from the linked object patterns.
//! See the [Implementing local behavior][local-behavior] section for details on thread-local logic.
//!
//! The implementation steps to apply the pattern to a struct are:
//!
//! * Add [`#[linked::object]`][crate::object] to the struct. This will automatically
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
//! struct itself does not need to be thread-safe. See the next chapter to understand how to
//! implement multithreaded logic.
//!
//! # Linked objects on multiple threads
//! [multiple-threads]: #multiple-threads
//!
//! Each instance of a linked object is expected to be optimized for thread-local logic. The purpose
//! of this pattern is to encourage highly efficient local operation while still presenting an easy
//! to use API to both the author and the user of the type.
//!
//! A single-threaded type is generally `!Send` and `!Sync`, which means it cannot be sent between
//! threads or used from other threads. Therefore, you cannot in the general case just clone an
//! instance to share it with a different thread. This poses an obvious question: how can we then
//! create different instances from the same family for different threads?
//!
//! This package provides several mechanisms for this:
//!
//! * [`linked::instances!`][1] will enrich static variables with linked object powers - you can
//!   use a static variable to get linked instances from the same object family;
//! * [`linked::thread_local_rc!`][2] takes it one step further and manages the bookkeeping necessary
//!   to only maintain one instance per thread, which you can access either via `&T` shared
//!   reference or obtain an `Rc<T>` to;
//! * [`linked::thread_local_arc!`][8] also manages one instance per thread but is designed for
//!   types where `T: Sync` and allows you to obtain an `Arc<T>`;
//! * [`linked::InstancePerThread<T>`][3] is roughly equivalent to `thread_local_rc!` but does
//!   not require you to define a static variable; this is useful when you do not know at compile
//!   time how many object families you need to create;
//! * [`linked::InstancePerThreadSync<T>`][9] is the same but equivalent to `thread_local_arc!`
//!   and requires `T: Sync`;
//! * [`linked::Family<T>`][11] is the lowest level primitive, being a handle to the object family
//!   that can be used to create new instances on demand using custom logic.
//!
//! Example of using a static variable to link instances on different threads:
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
//! linked::instances!(static THE_THING: Thing = Thing::new("hello".to_string()));
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
//! Example of using a [`InstancePerThread<T>`][3] to dynamically define an object family and
//! create thread-local instances on different threads:
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
//! use linked::InstancePerThread;
//!
//! let linked_thing = InstancePerThread::new(Thing::new("hello".to_string()));
//!
//! // Obtain a local instance on demand.
//! let thing = linked_thing.acquire();
//! assert_eq!(thing.value(), "hello");
//!
//! thing.set_value("world".to_string());
//!
//! thread::spawn({
//!     // The new thread gets its own clone of the InstancePerThread<T>.
//!     let linked_thing = linked_thing.clone();
//!
//!     move || {
//!         let thing = linked_thing.acquire();
//!         assert_eq!(thing.value(), "world");
//!     }
//! })
//! .join()
//! .unwrap();
//! ```
//!
//! Example of using a [`linked::Family`][11] to manually create an instance on a different thread:
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
//! use linked::Object; // This brings .family() into scope.
//!
//! let thing = Thing::new("hello".to_string());
//! assert_eq!(thing.value(), "hello");
//!
//! thing.set_value("world".to_string());
//!
//! thread::spawn({
//!     // You can get the object family from any instance.
//!     let thing_family = thing.family();
//!
//!     move || {
//!         // Use .into() to convert the family reference into a new instance.
//!         let thing: Thing = thing_family.into();
//!         assert_eq!(thing.value(), "world");
//!     }
//! })
//! .join()
//! .unwrap();
//! ```
//!
//! # Implementing local behavior
//! [local-behavior]: #local-behavior
//!
//! The linked object pattern does not change the fact that synchronized state is expensive.
//! Whenever possible, linked objects should operate on local state for optimal efficiency.
//!
//! Let us extend `Thing` from above with a local counter that counts the number of times the value
//! has been modified via the current instance. This is local behavior that does not require any
//! synchronization with other instances.
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
//!         self.update_count += 1;
//!     }
//!
//!     pub fn update_count(&self) -> usize {
//!         self.update_count
//!     }
//! }
//! ```
//!
//! **Local behavior consists of simply operating on regular non-synchronized fields of the struct.**
//!
//! The above implementation works well with some of the linked object mechanisms provided by this
//! crate, such as [`linked::instances!`][1] and [`linked::Family<T>`][11].
//!
//! However, the above implementation does not work with instance-per-thread mechanisms like
//! [`linked::thread_local_rc!`][2] because these mechanisms share one instance between many
//! callers on the same thread. This makes it impossible to obtain an exclusive `&mut self`
//! reference (as required by `set_value()`) because exclusive access cannot be guaranteed.
//!
//! Types designed to be used via instance-per-thread mechanisms cannot modify the local
//! state directly but must instead use interior mutability (e.g. [`Cell`][6] or [`RefCell`][7]).
//!
//! Example of the same type using [`Cell`][6] to support thread-local behavior without `&mut self`:
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
//!         self.update_count.set(self.update_count.get() + 1);
//!     }
//!
//!     pub fn update_count(&self) -> usize {
//!         self.update_count.get()
//!     }
//! }
//! ```
//!
//! # You may still need `Send` when thread-isolated
//!
//! There are some practical considerations that mean you often want your linked objects
//! to be `Send` and `Sync`, just like traditional thread-safe objects are.
//!
//! This may be surprising - after all, the whole point of this package is to enable thread-local
//! behavior, which does not require `Send` or `Sync`.
//!
//! The primary reason is that **many APIs in the Rust ecosystem require `Send` from types,
//! even in scenarios where the object is only accessed from a single thread**. This is because
//! [the language lacks the flexibility necessary to create APIs that support both `Send` and
//! `!Send` types][12], so many API authors simply require `Send` from all types.
//!
//! Implementing `Send` is therefore desirable for practical API compatibility reasons, even if the
//! type is never sent across threads.
//!
//! The main impact of this is that you want to avoid fields that are `!Send` in your linked
//! object types (e.g. the most common such type being `Rc`).
//!
//! # You may still need `Sync` when thread-isolated
//!
//! It is not only instances of linked objects themselves that may need to be passed around
//! to 3rd party APIs that require `Send` - you may also want to pass long-lived references to
//! the instance-per-thread linked objects (or references to your own structs that contain such
//! references). This can be common, e.g. when passing futures to async task runtimes, with
//! these references/types being stored as part of the future's async state machine.
//!
//! All linked object types support instance-per-thread behavior via [`linked::thread_local_rc!`][2]
//! and [`linked::InstancePerThread<T>`][3] but these give you `Rc<T>` and [`linked::Ref<T>`][12],
//! which are `!Send`. That will not work with many 3rd party APIs!
//!
//! Instead, you want to use [`linked::thread_local_arc!`][8] or
//! [`linked::InstancePerThreadSync<T>`][9], which give you `Arc<T>` and [`linked::RefSync<T>`][14],
//! which are both `Send`. This ensures compatibility with 3rd party APIs.
//!
//! Use of these two mechanisms requires `T: Sync`, however!
//!
//! Therefore, for optimal compatibility with 3rd party APIs, you will often want to design your
//! linked object types to be both `Send` and `Sync`, even if each instance is only used from a
//! single thread.
//!
//! Example extending the above example using [`AtomicUsize`][10] to become `Sync`:
//!
//! ```rust
//! use std::sync::atomic::{self, AtomicUsize};
//! use std::sync::{Arc, Mutex};
//!
//! #[linked::object]
//! pub struct Thing {
//!     // Shared state - synchronized with other instances in the family.
//!     value: Arc<Mutex<String>>,
//!
//!     // Local state - not synchronized with other instances in the family.
//!     update_count: AtomicUsize,
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
//!             update_count: AtomicUsize::new(0),
//!         })
//!     }
//!
//!     pub fn value(&self) -> String {
//!         self.value.lock().unwrap().clone()
//!     }
//!
//!     pub fn set_value(&self, value: String) {
//!         *self.value.lock().unwrap() = value;
//!         self.update_count.fetch_add(1, atomic::Ordering::Relaxed);
//!     }
//!
//!     pub fn update_count(&self) -> usize {
//!         self.update_count.load(atomic::Ordering::Relaxed)
//!     }
//! }
//! ```
//!
//! This has some nonzero overhead, so avoiding it is still desirable if you are operating in a
//! situation where 3rd party APIs do not require `Send` from your types. However, it is still
//! much more efficient than traditional thread-safe types because while it uses thread-safe
//! primitives like atomics, these are only ever accessed from a single thread, which is very fast.
//!
//! The underlying assumption of the performance claims is that you do not actually share a single
//! thread's instance with other threads, of course.
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
//! `dyn Xyz` form - you can clone the box, obtain a [`Family<linked::Box<dyn Xyz>>`][Family] to
//! extend the object family to another thread and store such a box in a static variable in a
//! [`linked::instances!`][1] or [`linked::thread_local_rc!`][2] block or a
//! [`linked::InstancePerThread<T>`][3] for automatic instance management.
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
//!     config: String,
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
//!     config: String,
//! }
//!
//! impl XmlConfig {
//!     // XmlConfig itself is a regular linked object, nothing special about it.
//!     pub fn new() -> XmlConfig {
//!         linked::new!(Self {
//!             config: "xml".to_string(),
//!         })
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
//! [1]: crate::instances
//! [2]: crate::thread_local_rc
//! [3]: crate::InstancePerThread
//! [4]: https://doc.rust-lang.org/nomicon/send-and-sync.html
//! [5]: std::boxed::Box
//! [6]: std::cell::Cell
//! [7]: std::cell::RefCell
//! [8]: crate::thread_local_arc
//! [9]: crate::InstancePerThreadSync
//! [10]: std::sync::atomic::AtomicUsize
//! [11]: crate::Family
//! [12]: https://github.com/rust-lang/rfcs/issues/2190
//! [13]: crate::Ref
//! [14]: crate::RefSync

use simple_mermaid::mermaid;

#[doc(hidden)]
pub mod __private;

mod r#box;
mod constants;
mod family;
mod instance_per_thread;
mod instance_per_thread_sync;
mod object;
mod static_instance_per_thread;
mod static_instance_per_thread_sync;
mod static_instances;
mod thread_id_hash;

pub use r#box::*;
pub(crate) use constants::*;
pub use family::*;
pub use instance_per_thread::*;
pub use instance_per_thread_sync::*;
pub use object::*;
pub use static_instance_per_thread::*;
pub use static_instance_per_thread_sync::*;
pub use static_instances::*;
pub(crate) use thread_id_hash::*;

mod macros;

/// Marks a struct as implementing the [linked object pattern][crate].
///
/// See package-level documentation for a high-level guide.
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
///         linked::new!(Self { some_value })
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
///    pattern mechanisms such as calling `.family()` on instances to access the [`Family`].
/// 4. Implements `From<linked::Family<T>>` for the struct. This allows converting a [`Family`]
///    into an instance of the linked object using `.into()`.
///
/// # Constraints
///
/// Only structs defined in the named fields form are supported (no tuple structs).
pub use linked_macros::__macro_linked_object as object;

// This is so procedural macros can produce code which refers to
// ::linked::* which will work also in the current crate.
#[doc(hidden)]
extern crate self as linked;
