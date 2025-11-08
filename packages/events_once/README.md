# Events Once

Efficient oneshot events (channels) with support for single-threaded events, object embedding, event pools and event lakes.

An event is a pair of a sender and receiver, where the sender can be used at most once. When the event occurs, the sender submits a payload to the receiver. Meanwhile, the receiver can await the arrival of the payload.

This package expands on basic oneshot functionality and provides additional features while maintaining high performance and low overhead:

- **Single-threaded events**: Events that can only be used from a single thread, allowing for lower overhead and better performance where thread-safety is not required.
- **Object embedding**: Events can be embedded directly within other objects instead of being allocated on the heap. This reduces allocation overhead and improves cache locality.
- **Event pools**: Reusable pools of events that can be recycled to reduce heap memory allocation overhead and improve performance in high-throughput scenarios.
- **Event lakes**: Event pools for heterogeneous event types, allowing for efficient management and processing of diverse events that carry different types of payloads whose types are not known in advance.

The events support both asynchronous awaiting and ad-hoc completion polling. Synchronous waiting for event completion is not supported.

## Example

```rust
use events_once::Event;

#[tokio::main]
async fn main() {
    let (sender, receiver) = Event::<String>::boxed();

    sender.send("Hello, world!".to_string());

    // Events are thread-safe by default and their endpoints
    // may be freely moved to other threads or tasks.
    tokio::spawn(async move {
        let message = receiver.await.unwrap();
        println!("{message}");
    })
    .await
    .unwrap();
}
```

More details in the [package documentation](https://docs.rs/events_once/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for high-performance hardware-aware programming in Rust.