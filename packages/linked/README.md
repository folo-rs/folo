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
use std::cell::Cell;
use std::sync::{Arc, Mutex};
use std::thread;

#[linked::object]
pub struct EventCounter {
    // Local state - not synchronized, very fast access
    local_count: Cell<usize>,
    
    // Shared state - synchronized across all instances in the family
    global_count: Arc<Mutex<usize>>,
}

impl EventCounter {
    pub fn new() -> Self {
        let global_count = Arc::new(Mutex::new(0));

        linked::new!(Self {
            local_count: Cell::new(0),
            global_count: Arc::clone(&global_count),
        })
    }

    pub fn record_event(&self) {
        // Fast thread-local increment
        self.local_count.set(self.local_count.get() + 1);
        
        // Synchronized global counter
        *self.global_count.lock().unwrap() += 1;
    }

    pub fn local_events(&self) -> usize {
        self.local_count.get()
    }

    pub fn total_events(&self) -> usize {
        *self.global_count.lock().unwrap()
    }
}

// Static variable provides linked instances across threads
linked::instances!(static EVENTS: EventCounter = EventCounter::new());

// Record events on main thread
let counter = EVENTS.get();
counter.record_event();
counter.record_event();

thread::spawn(|| {
    // Get a linked instance on another thread
    let counter = EVENTS.get();
    counter.record_event();
    
    assert_eq!(counter.local_events(), 1);  // Local to this thread
    assert_eq!(counter.total_events(), 3);  // Global across all instances
}).join().unwrap();

assert_eq!(counter.local_events(), 2);  // Still 2 on main thread
assert_eq!(counter.total_events(), 3);   // Global total visible everywhere
```

More details in the [package documentation](https://docs.rs/linked/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.