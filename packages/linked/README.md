Mechanisms for creating families of linked objects that can collaborate across threads,
with each instance only used from a single thread.

The problem this crate solves is that while writing highly efficient lock-free thread-local
code can yield great performance, it comes with serious drawbacks in terms of usability and
developer experience.

This crate bridges the gap by providing patterns and mechanisms that facilitate thread-local
behavior while presenting a simple and reasonably ergonomic API to user code:

* Internally, a linked object can take advantage of lock-free thread-isolated logic for **high
  performance and efficiency** because it operates as a multithreaded family of thread-isolated
  objects, each of which implements local behavior on a single thread.
* Externally, the linked object family can look and act very much like a single Rust object and
  can hide the fact that there is collaboration happening on multiple threads,
  providing **a reasonably simple API with minimal extra complexity** for both the author
  and the user of a type.

```rust
use std::sync::{Arc, Mutex};

#[linked::object]
pub struct Thing {
    value: Arc<Mutex<String>>,
}

impl Thing {
    pub fn new(initial_value: String) -> Self {
        let shared_value = Arc::new(Mutex::new(initial_value));

        linked::new!(Self {
            // Capture `shared_value` to reuse it for all instances in the family.
            value: Arc::clone(&shared_value),
        })
    }

    pub fn value(&self) -> String {
        self.value.lock().unwrap().clone()
    }

    pub fn set_value(&self, value: String) {
        *self.value.lock().unwrap() = value;
    }
}
```

More details in the [package documentation](https://docs.rs/linked/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.