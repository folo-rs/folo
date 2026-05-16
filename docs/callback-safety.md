# Callback safety

User-supplied callbacks (`Waker::wake`, `Drop` impls, observer closures, stored `FnOnce`
values, panics propagated through `catch_unwind`, etc.) can re-enter the data structure
they were invoked from. The corruption that results is logical, not memory-safety: the
borrow checker, Miri, and the type system can all be satisfied while a reentrant callback
silently breaks an invariant. Three rules apply repo-wide.

## 1. No callbacks under borrows of shared state

A user-supplied callback must never be invoked while a `&mut` borrow or lock on
caller-owned shared state is held. Release the borrow before the callback, then
re-acquire on the next iteration if needed.

Bad — the lock is held while the waker runs, which may deadlock or alias if the waker
re-enters:

```rust
let mut guard = state.lock().unwrap();
let waker = guard.take_waker();
waker.wake();
```

Good — release the lock before invoking the callback:

```rust
let waker = {
    let mut guard = state.lock().unwrap();
    guard.take_waker()
};
waker.wake();
```

The same rule applies to `UnsafeCell`-protected single-threaded state. A `&mut` borrow
through `UnsafeCell::get()` must be released before any user callback fires, otherwise
a reentrant access on the same thread creates an aliasing `&mut` that Miri flags as UB.

## 2. No `mem::take` / `mem::swap` / `mem::replace` on shared state if a callback can re-enter through the original location

The "snapshot and drain" anti-pattern is:

```rust
let snapshot = mem::take(&mut self.items);
for item in snapshot {
    item.callback();  // BUG: may mutate self.items, leaving snapshot stale.
}
```

Intrusive data structures (linked lists, slabs, doubly-linked sets) are particularly
fragile because internal pointers may cross between the snapshot and the original
location. A reentrant callback that operates on the "live" data structure observes a
fresh empty container, and any state changes it makes (registrations, removals) are
silently lost when the snapshot is dropped.

Prefer a rescan-from-head loop that operates on the live data structure:

```rust
loop {
    let item = {
        let mut guard = self.items.lock().unwrap();
        let Some(item) = guard.pop_front() else { break };
        item
    };
    item.callback();
}
```

`mem::replace` of a state field followed by a user callback is sound only when the
caller-visible data structure is fully consistent at the point the callback runs and
the callback cannot reach back through the same location. Document the chain of
reasoning inline; do not rely on the borrow checker alone, because raw-pointer paths
and interior mutability can route around it.

State-machine transitions that must be observed by reentrant code (such as transitioning
to a terminal state before invoking a wake callback) must complete *before* the
callback runs. Symmetric paths in the same primitive (set vs. disconnect, success vs.
failure) must use the same ordering.

## 3. Document reentrancy contracts on public async primitives

Every public async primitive must have a `# Reentrancy` section in its API docs that
spells out which reentrant operations from inside a `Waker::wake` callback are sound
(drop self, drop sibling, set, reset, register new, etc.). When adding a new async
primitive, ship matching reentrancy parity tests for every thread-safe and
single-threaded variant — at minimum: reentrant set, reentrant drop of self and
sibling, reentrant register during set, and any operation called out as sound in the
docs.
