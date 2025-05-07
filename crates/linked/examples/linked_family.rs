// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

//! This is a variation of `linked_basic.rs` - familiarize yourself with that example first.
//!
//! Demonstrates how to use linked objects across threads by manually establishing the linked
//! object family relationships via passing a reference to the family across threads and manually
//! creating instances from the family. This is useful because sometimes it might not be convenient
//! for you to define a static variable or use one of the standard instance-per-thread mechanisms.
//!
//! This example creates linked instances directly from the linked object family. This is
//! the most flexible approach but also requires the most code from you.

#![allow(clippy::new_without_default, reason = "Not relevant for example")]

use std::thread;

// This trait allows you to access the family of a linked object.
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

    // Every linked object belongs to a family, which you can access like this.
    // The family reference this returns is always thread-safe, even if the linked
    // object instances themselves are not. This allows you to pass it between threads.
    let counter_family = counter.family();

    for _ in 0..THREAD_COUNT {
        threads.push(thread::spawn({
            // We create a new clone of the family reference for each thread we spawn.
            let counter_family = counter_family.clone();

            move || {
                // The family reference can be converted to a new instance on demand.
                let mut counter: EventCounter = counter_family.into();

                for _ in 0..RECORDS_PER_THREAD {
                    counter.increment();
                }

                println!(
                    "Thread completed work; local count: {}, global count: {}",
                    counter.local_count(),
                    counter.global_count()
                );
            }
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }

    let final_count = counter.global_count();

    println!("All threads completed work; final global count: {final_count}");
}
