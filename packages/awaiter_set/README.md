# awaiter_set

Zero-allocation awaiter tracking for async synchronization primitives.

This crate provides two types:

- `AwaiterSet` — the set of registered awaiters, managed by the
  synchronization primitive.
- `Awaiter` — a single awaiter, embedded inside a future.

Each awaiter lives directly inside the awaiting future rather than
being heap-allocated, making registration and removal zero-allocation.

## Quick start

```rust
use awaiter_set::{Awaiter, AwaiterSet};

let mut set = AwaiterSet::new();
let mut a = Awaiter::new();
let mut b = Awaiter::new();

// SAFETY: Awaiters remain valid and pinned while in the set.
unsafe {
    a.register(&mut set, std::task::Waker::noop().clone());
    b.register(&mut set, std::task::Waker::noop().clone());
}

assert!(!set.is_empty());

if let Some(waker) = set.notify_one() {
    // Wake the awaiter outside the lock scope.
    drop(waker);
}
```

## Design

The set does not own its awaiters — each awaiter's lifetime is tied
to the future that contains it. Awaiters must remain at a stable
pinned address from registration until removal.

## Thread safety

`AwaiterSet` has no internal synchronization. Callers must provide
exclusive access:

* Thread-safe primitives protect all access with a `Mutex`.
* Single-threaded primitives constrain the containing type to `!Send`.

## License

MIT
