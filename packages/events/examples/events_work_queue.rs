//! Demonstrates using [`AutoResetEvent`] as a work-queue notification
//! mechanism with multiple producers and a single consumer.
//!
//! Each producer pushes a work item into a shared queue and signals the
//! consumer via `set()`. The consumer waits for a signal, then drains all
//! available items before waiting again.

use std::sync::{Arc, Mutex};

use events::AutoResetEvent;

const ITEMS_PER_PRODUCER: usize = 5;
const PRODUCER_COUNT: usize = 3;

#[tokio::main]
async fn main() {
    producer_consumer().await;
}

/// Multiple producers push items and signal; a single consumer drains
/// the queue on each wake-up.
async fn producer_consumer() {
    let event = AutoResetEvent::boxed();
    let queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Spawn producers.
    let mut handles = Vec::new();
    for id in 0..PRODUCER_COUNT {
        let event = event.clone();
        let queue = Arc::clone(&queue);
        handles.push(tokio::spawn(async move {
            for i in 0..ITEMS_PER_PRODUCER {
                let item = format!("producer-{id}/item-{i}");
                queue.lock().expect("not poisoned").push(item);
                event.set();
            }
        }));
    }

    // Consumer: collect all items until we have seen everything.
    let total_expected = PRODUCER_COUNT
        .checked_mul(ITEMS_PER_PRODUCER)
        .expect("product fits in usize because both factors are small");
    let mut collected = Vec::new();

    while collected.len() < total_expected {
        // Wait for at least one producer to signal.
        event.wait().await;

        // Drain whatever is available — multiple signals may have
        // coalesced into a single wake-up, which is fine.
        let mut locked = queue.lock().expect("not poisoned");
        collected.append(&mut locked);
    }

    // Wait for all producers to finish.
    for handle in handles {
        handle.await.expect("producer did not panic");
    }

    println!("Consumer received all {total_expected} items: {collected:?}");
}
