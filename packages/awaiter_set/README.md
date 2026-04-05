# awaiter_set

Zero-allocation awaiter tracking for async synchronization primitives.

This crate provides `AwaiterSet`, `AwaiterNode`, and `AwaiterNodeStorage`
for parking and waking async futures that await on synchronization events.
Each node is embedded directly inside the awaiting future rather than being
heap-allocated, making registration and removal zero-allocation.

## Quick start

```rust
use std::pin::Pin;

use awaiter_set::{AwaiterSet, AwaiterNode};

let mut set = AwaiterSet::new();
let mut node_a = AwaiterNode::new();
let mut node_b = AwaiterNode::new();

// SAFETY: Nodes remain valid and pinned while in the set.
unsafe {
    set.insert(Pin::new_unchecked(&mut node_a));
    set.insert(Pin::new_unchecked(&mut node_b));
}

assert!(!set.is_empty());

let first = set.take_one();
assert!(first.is_some());
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
