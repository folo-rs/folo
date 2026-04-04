# awaiter_set

Zero-allocation awaiter tracking for async synchronization primitives.

This crate provides `AwaiterSet`, `AwaiterNode`, and `AwaiterNodeStorage`
for parking and waking async futures that await on synchronization events.
Each node is embedded directly inside the awaiting future rather than being
heap-allocated, making registration and removal zero-allocation.

## Quick start

```rust
use awaiter_set::{AwaiterSet, AwaiterNode};

let mut set = AwaiterSet::new();
let mut node_a = AwaiterNode::new();
let mut node_b = AwaiterNode::new();

let pa: *mut AwaiterNode = &raw mut node_a;
let pb: *mut AwaiterNode = &raw mut node_b;

// SAFETY: Nodes are valid and not in any set.
unsafe {
    set.insert(pa);
    set.insert(pb);
}

// SAFETY: We have exclusive access.
assert!(!unsafe { set.is_empty() });

// SAFETY: We have exclusive access.
let first = unsafe { set.take_one() };
assert!(std::ptr::eq(first.unwrap(), pa));
```

## Design

The set does not own its nodes — each node's lifetime is tied to the
future that contains it. Nodes must remain at a stable pinned address
from insertion until removal.

## Thread safety

`AwaiterSet` has no internal synchronization. Callers must provide
exclusive access:

* Thread-safe primitives protect all access with a `Mutex`.
* Single-threaded primitives constrain the containing type to `!Send`.

## License

MIT
