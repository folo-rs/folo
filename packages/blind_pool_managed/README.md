# blind_pool_managed

[![Crates.io](https://img.shields.io/crates/v/blind_pool_managed.svg)](https://crates.io/crates/blind_pool_managed)
[![Documentation](https://docs.rs/blind_pool_managed/badge.svg)](https://docs.rs/blind_pool_managed)

A thread-safe wrapper around `BlindPool` with automatic resource management and reference counting.

This package provides `ManagedBlindPool`, which extends `BlindPool` with thread-safe, cloneable handles and automatic cleanup of pooled items when they are no longer referenced.

## Features

- **Thread-safe**: Built on top of `Arc<Mutex<BlindPool>>` for safe concurrent access
- **Cloneable handles**: The pool itself acts as a lightweight handle that can be cloned freely
- **Automatic cleanup**: Items are automatically removed from the pool when the last reference is dropped
- **Reference counting**: Uses `Arc` internally to manage the lifetime of pooled items
- **Type safety**: Maintains all the type safety guarantees of the underlying `BlindPool`
- **No manual removal**: No need to explicitly remove items - they are cleaned up automatically

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
blind_pool_managed = "0.1.0"
```

## Basic Example

```rust
use blind_pool::BlindPool;
use blind_pool_managed::ManagedBlindPool;

// Create a managed pool from a regular BlindPool.
let pool = ManagedBlindPool::from(BlindPool::new());

// The pool can be cloned and shared across threads.
let pool_clone = pool.clone();

// Insert values and get managed handles.
let managed_u32 = pool.insert(42_u32);
let managed_string = pool.insert("hello".to_string());

// Access values through dereferencing.
assert_eq!(*managed_u32, 42);
assert_eq!(*managed_string, "hello");

// Values are automatically removed when all references are dropped.
drop(managed_u32);
drop(managed_string);
```

## Thread Safety

```rust
use std::thread;
use blind_pool::BlindPool;
use blind_pool_managed::ManagedBlindPool;

let pool = ManagedBlindPool::from(BlindPool::new());
let pool_clone = pool.clone();

let handle = thread::spawn(move || {
    let managed_item = pool_clone.insert(42_u64);
    *managed_item
});

let value = handle.join().unwrap();
assert_eq!(value, 42);
```

## Reference Counting

```rust
use blind_pool::BlindPool;
use blind_pool_managed::ManagedBlindPool;

let pool = ManagedBlindPool::from(BlindPool::new());

let managed_value = pool.insert(42_u64);
let cloned_handle = managed_value.clone();

// Both handles refer to the same value.
assert_eq!(*managed_value, *cloned_handle);

// Value remains in pool until all handles are dropped.
drop(managed_value);
assert_eq!(*cloned_handle, 42); // Still accessible.

drop(cloned_handle); // Now the value is removed from the pool.
```

## API Overview

### `ManagedBlindPool`

- `from(pool: BlindPool) -> Self` - Create from an existing `BlindPool`
- `insert<T>(&self, value: T) -> ManagedPooled<T>` - Insert a value and get a managed handle
- `len(&self) -> usize` - Get the number of items in the pool
- `is_empty(&self) -> bool` - Check if the pool is empty
- `Clone` - Clone the pool handle

### `ManagedPooled<T>`

- `ptr(&self) -> NonNull<T>` - Get a raw pointer to the value
- `erase(self) -> ManagedPooled<()>` - Erase type information
- `Deref<Target = T>` - Access the value directly
- `Clone` - Clone the handle (increases reference count)
- Automatic `Drop` - Removes from pool when last reference is dropped

## Relationship to `BlindPool`

`ManagedBlindPool` builds on top of `BlindPool`, adding:

1. **Thread safety** through `Arc<Mutex<BlindPool>>`
2. **Automatic resource management** through reference counting
3. **Cloneable pool handles** for easy sharing
4. **No need for manual cleanup** - items are removed automatically

All the performance characteristics and type safety of `BlindPool` are preserved, with the added convenience of automatic memory management.

## When to Use

Use `ManagedBlindPool` when you need:

- Thread-safe access to a blind pool
- Automatic cleanup of pooled items
- Easy sharing of the pool across multiple parts of your application
- Reference counting semantics for pooled items

Use the regular `BlindPool` when you need:

- Maximum performance (no Arc/Mutex overhead)
- Manual control over item lifetimes
- Single-threaded access patterns

## License

Licensed under the [MIT license](LICENSE).
