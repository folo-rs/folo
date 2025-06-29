//! Example demonstrating multiple threads operating on shared data structure, performing different actions.
//!
//! This scenario shows two workers where one acts as a producer/sender and the other as a
//! consumer/receiver. They communicate through channels, representing a common pattern where
//! different threads have complementary roles in processing data.

#![allow(
    missing_docs,
    clippy::let_underscore_must_use,
    reason = "No need for API documentation in example code"
)]

use std::hint::black_box;
use std::sync::mpsc;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    // Workers perform different actions, so all distribution modes are relevant
    // Use a smaller batch size since channel operations are relatively fast
    execute_runs::<ProducerConsumerChannels, 500>(
        c,
        WorkDistribution::all_with_unique_processors(),
    );
}

/// Producer-consumer pattern using channels for inter-thread communication.
///
/// This demonstrates the "different actions" scenario where threads have complementary roles.
/// One worker produces data (acting as sender), while the other consumes it (acting as receiver).
/// The performance differences between work distribution modes will show how memory locality
/// affects message passing overhead.
#[derive(Debug)]
struct ProducerConsumerChannels {
    /// Channel for receiving messages from the partner worker.
    rx: mpsc::Receiver<u64>,

    /// Channel for sending messages to the partner worker.
    tx: mpsc::Sender<u64>,

    /// Determines this worker's role: true = primarily producer, false = primarily consumer.
    is_primary_producer: bool,
}

impl Payload for ProducerConsumerChannels {
    fn new_pair() -> (Self, Self) {
        // Create bidirectional channels for communication
        let (producer_tx, consumer_rx) = mpsc::channel::<u64>();
        let (consumer_tx, producer_rx) = mpsc::channel::<u64>();

        // Worker 1: Primary producer (sends more than it receives)
        let worker1 = Self {
            rx: producer_rx,
            tx: producer_tx,
            is_primary_producer: true,
        };

        // Worker 2: Primary consumer (receives more than it sends)
        let worker2 = Self {
            rx: consumer_rx,
            tx: consumer_tx,
            is_primary_producer: false,
        };

        (worker1, worker2)
    }

    fn prepare(&mut self) {
        // Pre-seed the channels with some initial messages to avoid deadlocks
        // and create a steady flow of data for benchmarking
        const INITIAL_MESSAGE_COUNT: usize = 5000;

        for i in 0..INITIAL_MESSAGE_COUNT {
            // Both workers send some initial messages
            self.tx.send(i as u64).unwrap();
        }
    }

    fn process(&mut self) {
        const OPERATION_COUNT: usize = 25000;

        if self.is_primary_producer {
            // Primary producer: mostly sends, occasionally receives
            for i in 0..OPERATION_COUNT {
                // Send a message
                _ = self.tx.send(i as u64);

                // Occasionally receive a message to keep the flow balanced
                if i % 10 == 0 {
                    if let Ok(received) = self.rx.try_recv() {
                        black_box(received);
                    }
                }
            }
        } else {
            // Primary consumer: mostly receives, occasionally sends
            for i in 0..OPERATION_COUNT {
                // Try to receive a message
                if let Ok(received) = self.rx.try_recv() {
                    // Process the received data
                    let processed_value = received.wrapping_mul(2);
                    black_box(processed_value);

                    // Occasionally send back a response
                    if i % 5 == 0 {
                        _ = self.tx.send(processed_value);
                    }
                }
            }
        }
    }
}
