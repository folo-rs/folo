A pinned object pool that contains any type of object as long as it has a compatible memory layout.

```rust
use std::alloc::Layout;

use opaque_pool::OpaquePool;

// Create a pool for storing values that match the layout of `u64`.
let layout = Layout::new::<u64>();
let mut pool = OpaquePool::builder().layout(layout).build();

// Insert values into the pool.
// SAFETY: The layout of u64 matches the pool's item layout.
let pooled1 = unsafe { pool.insert(42_u64) };
// SAFETY: The layout of u64 matches the pool's item layout.
let pooled2 = unsafe { pool.insert(123_u64) };

// Read data back from the pooled items.
let value1 = unsafe {
    // SAFETY: The pointer is valid and the value was just inserted.
    pooled1.ptr().read()
};
```

More details in the [package documentation](https://docs.rs/opaque_pool/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.