High-performance event signaling primitives for concurrent environments.

This package provides lightweight, efficient signaling mechanisms for communicating between
different parts of an application. The API is designed to be simple to use while offering
high performance in concurrent scenarios.

Both single-threaded and thread-safe variants are available for events and pools:
- `OnceEvent<T>`, `OnceSender<E>`, `OnceReceiver<E>` - Thread-safe event variants
- `LocalOnceEvent<T>`, `LocalOnceSender<E>`, `LocalOnceReceiver<E>` - Single-threaded event variants
- `OnceEventPool<T>`, `PooledOnceSender<P>`, `PooledOnceReceiver<P>` - Thread-safe pool variants
- `LocalOnceEventPool<T>`, `PooledLocalOnceSender<P>`, `PooledLocalOnceReceiver<P>` - Single-threaded pool variants

```rust
use events::OnceEvent;

// Create a thread-safe event for passing a string message
let event = OnceEvent::<String>::new();
let (sender, receiver) = event.bind_by_ref();

// Send a message through the event
sender.send("Hello, World!".to_string());

// Receive the message (await since it's async)
let message = receiver.await.unwrap();
assert_eq!(message, "Hello, World!");
```

More details in the [package documentation](https://docs.rs/events/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.