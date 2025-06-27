Efficiently schedule work and inspect the hardware environment on many-processor systems.

On systems with 100+ logical processors, taking direct control over work placement can yield 
superior performance by ensuring data locality and avoiding expensive cross-processor transfers.

```rust
use many_cpus::ProcessorSet;

let threads = ProcessorSet::default().spawn_threads(|processor| {
    println!("Spawned thread on processor {}", processor.id());

    // In a real service, you would start some work handler here, e.g. to read
    // and process messages from a channel or to spawn a web handler.
});
```

More details in the [package documentation](https://docs.rs/many_cpus/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.