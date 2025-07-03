//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use linked objects for creating families of linked objects
//! that can collaborate across threads while being internally single-threaded.

#![allow(
    missing_debug_implementations,
    missing_docs,
    clippy::new_without_default,
    clippy::must_use_candidate,
    clippy::mutex_atomic,
    clippy::arithmetic_side_effects,
    reason = "example code"
)]

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

fn main() {
    println!("=== Linked README Example ===");

    // Record events on main thread
    let counter = EVENTS.get();
    counter.record_event();
    counter.record_event();

    thread::spawn(|| {
        // Get a linked instance on another thread
        let counter = EVENTS.get();
        counter.record_event();

        assert_eq!(counter.local_events(), 1); // Local to this thread
        assert_eq!(counter.total_events(), 3); // Global across all instances
    })
    .join()
    .unwrap();

    assert_eq!(counter.local_events(), 2); // Still 2 on main thread
    assert_eq!(counter.total_events(), 3); // Global total visible everywhere

    println!("Main thread local events: {}", counter.local_events());
    println!(
        "Total events across all threads: {}",
        counter.total_events()
    );
    println!("README example completed successfully!");
}
