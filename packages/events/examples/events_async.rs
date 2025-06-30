//! Demonstrates async usage of events.
//!
//! This example shows how to use the async receive functionality
//! of both thread-safe and single-threaded events.

use std::thread;
use std::time::Duration;

use events::{LocalOnceEvent, OnceEvent};
use futures::executor::block_on;

/// Example integer value for testing.
const EXAMPLE_INTEGER: i32 = 42;

/// Example values for async testing.
const FIRST_VALUE: u64 = 100;
const SECOND_VALUE: u64 = 200;

/// Simulated work duration in milliseconds.
const SIMULATED_WORK_DURATION_MS: u64 = 100;

fn basic_async_usage() {
    println!("1. Basic async usage with thread-safe events:");
    block_on(async {
        let event = OnceEvent::<String>::new();
        let (sender, receiver) = event.bind_by_ref();

        sender.send("Hello async world!".to_string());
        let message = receiver.await.expect(
            "sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received: {message}");
    });
}

fn cross_thread_async_communication() {
    println!("2. Cross-thread async communication:");

    // For cross-thread communication, we need to extract endpoints before threading
    let event = OnceEvent::<i32>::new();
    let (sender, receiver) = event.bind_by_ref();

    // Use scoped threads to ensure all operations complete before event is dropped
    thread::scope(|s| {
        let sender_handle = s.spawn(move || {
            // Simulate some work
            thread::sleep(Duration::from_millis(SIMULATED_WORK_DURATION_MS));
            sender.send(EXAMPLE_INTEGER);
            println!("Sent value from background thread");
        });

        let receiver_handle = s.spawn(move || {
            let value = block_on(receiver)
                .expect("sender thread is guaranteed to call send() before completing");
            println!("Received value {value} in background thread");
            value
        });

        sender_handle.join().expect("Sender thread should complete");
        let received_value = receiver_handle
            .join()
            .expect("Receiver thread should complete");
        assert_eq!(received_value, EXAMPLE_INTEGER);
    });
}

fn single_threaded_async_usage() {
    println!("3. Single-threaded async usage:");
    block_on(async {
        let local_event = LocalOnceEvent::<String>::new();
        let (local_sender, local_receiver) = local_event.bind_by_ref();

        local_sender.send("Local async message".to_string());
        let local_message = local_receiver
            .await
            .expect("local_sender.send() was called immediately before this");
        println!("Received: {local_message}");
    });
}

fn async_only_usage() {
    println!("4. Async-only usage:");
    block_on(async {
        let first_event = OnceEvent::<u64>::new();
        let (first_sender, first_receiver) = first_event.bind_by_ref();

        let second_event = OnceEvent::<u64>::new();
        let (second_sender, second_receiver) = second_event.bind_by_ref();

        first_sender.send(FIRST_VALUE);
        second_sender.send(SECOND_VALUE);

        // Use async receive for both
        let sync_value = first_receiver
            .await
            .expect("first_sender.send() was called immediately before this");
        let async_value = second_receiver
            .await
            .expect("second_sender.send() was called immediately before this");

        println!("First received: {sync_value}, Second received: {async_value}");
    });
}

fn main() {
    println!("Event async examples");

    basic_async_usage();
    println!();

    cross_thread_async_communication();
    println!();

    single_threaded_async_usage();
    println!();

    async_only_usage();

    println!();
    println!("All async examples completed successfully!");
}
