A pinned object pool that can store objects of any type.

The pool returns super-powered pointers (`Pooled<T>`) that can be copied and cloned freely, providing type-safe access to the stored values.

```rust
use blind_pool::BlindPool;

// Create a pool with automatic cleanup (recommended).
let pool = BlindPool::builder().build();

// Insert different types, get handles with automatic dereferencing.
let u32_handle = pool.insert(42_u32);
let string_handle = pool.insert("hello".to_string());

// Access values through automatic dereferencing.
assert_eq!(*u32_handle, 42);
assert_eq!(*string_handle, "hello");

// Clone handles freely - values stay alive as long as any handle exists.
let cloned = u32_handle.clone();
assert_eq!(*cloned, 42);

// Raw pointer access for advanced use cases.
// SAFETY: Pointer is valid for inserted value.
let raw_value = unsafe { u32_handle.ptr().read() };
assert_eq!(raw_value, 42);

// Automatic cleanup when all handles are dropped - no manual cleanup needed!
```

More details in the [package documentation](https://docs.rs/blind_pool/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.