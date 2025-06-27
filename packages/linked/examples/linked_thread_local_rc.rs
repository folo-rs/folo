// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

//! This is a variation of `linked_basic.rs` - familiarize yourself with that example first.
//!
//! Demonstrates how to share thread-local instances of linked objects, so all callers on a single
//! thread access the same instance of the linked object.

#![allow(clippy::new_without_default, reason = "Not relevant for example")]

use std::thread;

mod counters {
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[linked::object]
    pub(crate) struct EventCounter {
        // This now acts as a thread-local count because we only access a single instance of the
        // linked object on every thread.
        //
        // Multiple callers on a single thread using the same instance means they cannot use `&mut`
        // references, so we cannot have any function in our `impl` block that takes `&mut self`!
        // That requires interior mutability to be used for any local state changes, which is why
        // we use a Cell here to facilitate incrementing the local count.
        local_count: Cell<usize>,

        global_count: Arc<AtomicUsize>,
    }

    impl EventCounter {
        pub(crate) fn new() -> Self {
            let global_count = Arc::new(AtomicUsize::new(0));

            linked::new!(Self {
                local_count: Cell::new(0),
                global_count: Arc::clone(&global_count),
            })
        }

        // Note how this is `&self` instead of `&mut self` - we cannot use `&mut self` or have any
        // variables typed `mut EventCounter` or `&mut EventCounter` if we are reusing the same
        // instance for all operations aligned to a single thread.
        pub(crate) fn increment(&self) {
            self.local_count
                .set(self.local_count.get().saturating_add(1));
            self.global_count.fetch_add(1, Ordering::Relaxed);
        }

        pub(crate) fn local_count(&self) -> usize {
            self.local_count.get()
        }

        pub(crate) fn global_count(&self) -> usize {
            self.global_count.load(Ordering::Relaxed)
        }
    }
}

use counters::EventCounter;

linked::thread_local_rc!(static RECORDS_PROCESSED: EventCounter = EventCounter::new());

fn main() {
    const THREAD_COUNT: usize = 4;
    const INCREMENT_ITERATIONS: usize = 1_000;

    let mut threads = Vec::with_capacity(THREAD_COUNT);

    for _ in 0..THREAD_COUNT {
        threads.push(thread::spawn(move || {
            // This is the simplest approach, directly referencing the current thread's instance.
            RECORDS_PROCESSED.with(|x| x.increment());

            // If needed, you can also obtain a long-lived reference to the current thread's
            // instance. Obtaining a long-lived reference is more efficient when accessing the
            // thread-specific instance, as long as you actually reuse the reference.
            //
            // These two are the exact same instance, just accessed via different references.
            let counter1 = RECORDS_PROCESSED.to_rc();
            let counter2 = RECORDS_PROCESSED.to_rc();

            for _ in 0..INCREMENT_ITERATIONS {
                counter1.increment();
                counter2.increment();
            }

            // Again, the exact same instance as above!
            let counter3 = RECORDS_PROCESSED.to_rc();

            println!(
                "Thread completed work; thread local count: {}, global count: {}",
                counter3.local_count(),
                counter3.global_count()
            );
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }

    let final_count = RECORDS_PROCESSED.to_rc().global_count();

    println!("All threads completed work; final global count: {final_count}");
}
