# folo_hw
Hardware awareness for Rust apps.

Spawn threads pinned to processors:

```rust
let threads = folo_hw::pinned_threads::spawn(
    Instantiation::PerProcessor,
    PinScope::PerProcessor,
    thread_entrypoint
);
```

# Responding to changes in hardware configuration

The crate does not support detecting and responding to changes in the hardware environment at
runtime. For example, if the set of active processors changes at runtime (e.g. due to a change in
hardware or OS configuration) then this change might not be visible to the crate and some threads
may execute unexpected processors or fail to execute entirely.