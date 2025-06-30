High-performance event signaling primitives for concurrent environments.

(DRAFT API WITH PLACEHOLDER IMPLEMENTATION - WORK IN PROGRESS)

This crate provides lightweight, efficient signaling mechanisms for communicating between
different parts of an application. The API is designed to be simple to use while offering
high performance in concurrent scenarios.

Both single-threaded and thread-safe variants are available:
- `OnceEvent<T>`, `ByRefOnceSender<T>`, `ByRefOnceReceiver<T>` - Thread-safe variants
- `LocalOnceEvent<T>`, `ByRefLocalOnceSender<T>`, `ByRefLocalOnceReceiver<T>` - Single-threaded variants

```rust
use events::OnceEvent;

// Create a thread-safe event for passing a string message
let event = OnceEvent::<String>::new();
let (sender, receiver) = event.by_ref();

// Send a message through the event
sender.send("Hello, World!".to_string());

// Receive the message (await since it's async)
let message = receiver.await;
assert_eq!(message, "Hello, World!");
```

More details in the [package documentation](https://docs.rs/events/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.