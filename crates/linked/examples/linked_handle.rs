// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

//! This is a variation of `linked_basic.rs` - familiarize yourself with that example first.
//!
//! Demonstrates how to use linked objects across threads without using the `link!` macro. This is
//! useful because sometimes it might not be convenient for you to define a static variable. This
//! example instead creates linked instances from handles that are passed between threads.

#![allow(clippy::new_without_default, reason = "Not relevant for example")]

use std::thread;

// This trait provides the methods for obtaining handles from linked objects.
use linked::Object;

// Everything in the "counters" module is the same as in `linked_basic.rs`.
// The difference is all in main() below.
mod counters {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[linked::object]
    pub(crate) struct EventCounter {
        local_count: usize,
        global_count: Arc<AtomicUsize>,
    }

    impl EventCounter {
        pub(crate) fn new() -> Self {
            let global_count = Arc::new(AtomicUsize::new(0));

            linked::new!(Self {
                local_count: 0,
                global_count: Arc::clone(&global_count),
            })
        }

        pub(crate) fn increment(&mut self) {
            self.local_count = self.local_count.saturating_add(1);
            self.global_count.fetch_add(1, Ordering::Relaxed);
        }

        pub(crate) fn local_count(&self) -> usize {
            self.local_count
        }

        pub(crate) fn global_count(&self) -> usize {
            self.global_count.load(Ordering::Relaxed)
        }
    }
}

use counters::EventCounter;

fn main() {
    const THREAD_COUNT: usize = 4;
    const RECORDS_PER_THREAD: usize = 1_000;

    let mut threads = Vec::with_capacity(THREAD_COUNT);

    // We create the counter as a local variable here. Linked objects are
    // regular structs and are not limited to static variables in any way.
    let counter = EventCounter::new();

    for _ in 0..THREAD_COUNT {
        // While linked objects themselves are always single-threaded objects (you would get a
        // compile error if you tried to pass `counter.clone()`), we can take thread-safe
        // handles from them. This mechanism is provided by `folo::linked::Linked` (which must
        // be imported) for all types that implement the linked object pattern.
        let counter_handle = counter.handle();

        threads.push(thread::spawn(move || {
            // We can convert the handle back into a linked object in the new thread.
            let mut counter: EventCounter = counter_handle.into();

            for _ in 0..RECORDS_PER_THREAD {
                counter.increment();
            }

            println!(
                "Thread completed work; local count: {}, global count: {}",
                counter.local_count(),
                counter.global_count()
            );
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }

    let final_count = counter.global_count();

    println!("All threads completed work; final global count: {final_count}");
}
