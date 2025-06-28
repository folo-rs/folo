//! Demonstrates async usage of events.
//!
//! This example shows how to use the async receive functionality
//! of both thread-safe and single-threaded events.

use std::thread;
use std::time::Duration;

use events::once::{Event, LocalEvent};
use futures::executor::block_on;

fn basic_async_usage() {
    println!("1. Basic async usage with thread-safe events:");
    let event = Event::<String>::new();
    let (sender, receiver) = event.by_ref();

    sender.send("Hello async world!".to_string());
    let message = block_on(receiver.recv_async());
    println!("Received: {message}");
}

fn cross_thread_async_communication() {
    println!("2. Cross-thread async communication:");

    // For cross-thread communication, we need to extract endpoints before threading
    let event = Event::<i32>::new();
    let (sender, receiver) = event.by_ref();

    // Use scoped threads to ensure all operations complete before event is dropped
    thread::scope(|s| {
        let sender_handle = s.spawn(move || {
            // Simulate some work
            thread::sleep(Duration::from_millis(100));
            sender.send(42);
            println!("Sent value from background thread");
        });

        let receiver_handle = s.spawn(move || {
            let value = block_on(receiver.recv_async());
            println!("Received value {value} in background thread");
            value
        });

        sender_handle.join().expect("Sender thread should complete");
        let received_value = receiver_handle
            .join()
            .expect("Receiver thread should complete");
        assert_eq!(received_value, 42);
    });
}

fn single_threaded_async_usage() {
    println!("3. Single-threaded async usage:");
    let local_event = LocalEvent::<String>::new();
    let (local_sender, local_receiver) = local_event.by_ref();

    local_sender.send("Local async message".to_string());
    let local_message = block_on(local_receiver.recv_async());
    println!("Received: {local_message}");
}

fn mixing_sync_and_async() {
    println!("4. Mixing sync and async:");
    let sync_event = Event::<u64>::new();
    let (sync_sender, sync_receiver) = sync_event.by_ref();

    let async_event = Event::<u64>::new();
    let (async_sender, async_receiver) = async_event.by_ref();

    sync_sender.send(100);
    async_sender.send(200);

    // Use sync receive for one, async for the other
    let sync_value = sync_receiver.recv();
    let async_value = block_on(async_receiver.recv_async());

    println!("Sync received: {sync_value}, Async received: {async_value}");
}

fn main() {
    println!("Event async examples");

    basic_async_usage();
    println!();

    cross_thread_async_communication();
    println!();

    single_threaded_async_usage();
    println!();

    mixing_sync_and_async();

    println!();
    println!("All async examples completed successfully!");
}
