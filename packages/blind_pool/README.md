A pinned object pool that can store objects of any type.

The pool returns super-powered pointers (`Pooled<T>`) that can be copied and cloned freely, providing type-safe access to the stored values.

```rust
use blind_pool::BlindPool;

// Create a blind pool that can store any type.
let mut pool = BlindPool::new();

// Insert values of different types into the same pool.
let pooled_u64 = pool.insert(42_u64);
let pooled_i32 = pool.insert(-123_i32);
let pooled_f32 = pool.insert(2.71_f32);

// The handles act like super-powered pointers - they can be copied freely.
let pooled_u64_copy = pooled_u64;

// Read data back from the pooled items.
let value_u64 = unsafe {
    // SAFETY: The pointer is valid and the value was just inserted.
    pooled_u64.ptr().read()
};

let value_u64_copy = unsafe {
    // SAFETY: Both handles refer to the same stored value.
    pooled_u64_copy.ptr().read()
};

let value_i32 = unsafe {
    // SAFETY: The pointer is valid and the value was just inserted.
    pooled_i32.ptr().read()
};

assert_eq!(value_u64, 42);
assert_eq!(value_u64_copy, 42);
assert_eq!(value_i32, -123);
```

More details in the [package documentation](https://docs.rs/blind_pool/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.