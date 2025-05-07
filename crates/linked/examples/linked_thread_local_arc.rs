// Copyright (c) Folo authors.

//! This is a variation of `linked_basic.rs` - familiarize yourself with that example first.
//!
//! Demonstrates how to share thread-local instances of linked objects, so all callers on a single
//! thread access the same instance of the linked object, while still allowing the instances to
//! be moved across threads if the need arises.

#![allow(clippy::new_without_default, reason = "Not relevant for example")]

use std::thread;

mod counters {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[linked::object]
    pub(crate) struct EventCounter {
        // This now acts as a thread-aligned count because we only access a single instance of the
        // linked object when aligned to one particular thread.
        //
        // While the thread_local_arc! macro always maintains unique instances for every thread,
        // user code may still move the `Arc<T>` to a different thread, which is why we need to
        // make this type thread-safe.
        local_count: AtomicUsize,

        global_count: Arc<AtomicUsize>,
    }

    impl EventCounter {
        pub(crate) fn new() -> Self {
            let global_count = Arc::new(AtomicUsize::new(0));

            linked::new!(Self {
                local_count: AtomicUsize::new(0),
                global_count: Arc::clone(&global_count),
            })
        }

        // Note how this is `&self` instead of `&mut self` - we cannot use `&mut self` or have any
        // variables typed `mut EventCounter` or `&mut EventCounter` if we are reusing the same
        // instance for all operations aligned to a single thread.
        pub(crate) fn increment(&self) {
            self.local_count.fetch_add(1, Ordering::Relaxed);
            self.global_count.fetch_add(1, Ordering::Relaxed);
        }

        pub(crate) fn local_count(&self) -> usize {
            self.local_count.load(Ordering::Relaxed)
        }

        pub(crate) fn global_count(&self) -> usize {
            self.global_count.load(Ordering::Relaxed)
        }
    }
}

use counters::EventCounter;

linked::thread_local_arc!(static RECORDS_PROCESSED: EventCounter = EventCounter::new());

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
            let counter1 = RECORDS_PROCESSED.to_arc();
            let counter2 = RECORDS_PROCESSED.to_arc();

            for _ in 0..INCREMENT_ITERATIONS {
                counter1.increment();
                counter2.increment();
            }

            // Again, the exact same instance as above!
            let counter3 = RECORDS_PROCESSED.to_arc();

            println!(
                "Thread completed work; thread local count: {}, global count: {}",
                counter3.local_count(),
                counter3.global_count()
            );

            // You are allowed to send the `Arc<T>` across threads if you wish, but the T inside
            // will still be the one from the original thread it was created on and operate on the
            // local state of the original thread.
            thread::spawn(move || {
                println!(
                    "Observing from a new thread; thread local count: {}, global count: {}",
                    counter3.local_count(),
                    counter3.global_count()
                );
            })
            .join()
            .unwrap();
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }

    let final_count = RECORDS_PROCESSED.to_arc().global_count();

    println!("All threads completed work; final global count: {final_count}");
}
