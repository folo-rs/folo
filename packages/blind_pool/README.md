A pinned object pool that can store objects of any type.

The pool returns super-powered pointers (`Pooled<T>`) that can be copied and cloned freely, providing type-safe access to the stored values. A key feature is the ability to cast these pointers to trait objects while preserving reference counting semantics.

## Quick start

```rust
use blind_pool::BlindPool;

// Create a pool with automatic cleanup and thread safety.
let pool = BlindPool::new();

// Insert different types, get handles with automatic dereferencing.
let u32_handle = pool.insert(42_u32);
let string_handle = pool.insert("hello".to_string());

// Access values through automatic dereferencing.
assert_eq!(*u32_handle, 42);
assert_eq!(*string_handle, "hello");

// Clone handles freely - values stay alive as long as any handle exists.
let cloned = u32_handle.clone();
assert_eq!(*cloned, 42);

// Automatic cleanup when all handles are dropped - no manual cleanup needed!
```

## Trait object support

```rust
use blind_pool::{BlindPool, define_pooled_dyn_cast};
use std::fmt::Display;

// Enable casting to Display trait objects
define_pooled_dyn_cast!(Display);

let pool = BlindPool::new();

// Insert different types that implement Display
let int_handle = pool.insert(123_i32);
let string_handle = pool.insert("world".to_string());

// Cast to trait objects while preserving reference counting
let int_display = int_handle.cast_display();
let string_display = string_handle.cast_display();

// Use them uniformly through the trait
fn print_value(value: &dyn Display) {
    println!("Value: {}", value);
}

print_value(&*int_display);
print_value(&*string_display);
```

More details in the [package documentation](https://docs.rs/blind_pool/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.