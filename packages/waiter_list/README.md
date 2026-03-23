# waiter_list

Intrusive doubly-linked waiter queue for async synchronization primitives.

This crate provides `WaiterList` and `WaiterNode`, a FIFO queue designed for parking and waking
async futures that wait on synchronization events. The list is *intrusive*: each node is embedded
directly inside the wait future rather than being heap-allocated, making registration and removal
zero-allocation.

## Quick start

```rust
use waiter_list::{WaiterList, WaiterNode};

// Nodes are typically embedded in futures via UnsafeCell.
// This example shows the basic API.
let mut list = WaiterList::new();
let mut node_a = WaiterNode::new();
let mut node_b = WaiterNode::new();

let pa: *mut WaiterNode = &raw mut node_a;
let pb: *mut WaiterNode = &raw mut node_b;

// SAFETY: Nodes are valid and not in any list.
unsafe {
    list.push_back(pa);
    list.push_back(pb);
}

assert!(!list.is_empty());

// SAFETY: We have exclusive access to the list.
let first = unsafe { list.pop_front() };
assert!(std::ptr::eq(first.unwrap(), pa));
```

## Design

The list is a doubly-linked list with `head` and `tail` pointers. All operations are O(1).
The list does not own its nodes — each node's lifetime is tied to the future that contains it.

Nodes contain a `PhantomPinned` marker to enforce the pinning contract at the type level.
The list holds raw pointers to nodes, so nodes must remain at a stable address from the time
they are pushed until they are removed.

## Thread safety

`WaiterList` has no internal synchronization. Callers must provide exclusive access:

* Thread-safe primitives protect all access with a `Mutex`.
* Single-threaded primitives constrain the containing type to `!Send`.

## License

MIT
