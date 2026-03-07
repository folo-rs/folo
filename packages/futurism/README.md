# futurism

Utilities for working with futures, providing collection types for managing groups of futures
with precise control over polling order and result retrieval.

## Types

* `FutureDeque<T>` — a thread-mobile (`Send`) deque of futures. Requires inserted futures to
  be `Send`.
* `LocalFutureDeque<T>` — a single-threaded (`!Send`) variant that accepts any future.

Both types poll active futures in deterministic front-to-back order and allow results to be
popped from either end with strict deque semantics (only the actual front or back item can
be popped, and only if it has completed).

Both implement `Stream`, yielding completed results from the front.

## Example

```rust
use futures::{executor::block_on, StreamExt};
use futurism::LocalFutureDeque;

block_on(async {
    let mut deque = LocalFutureDeque::new();

    deque.push_back(async { 1 });
    deque.push_back(async { 2 });
    deque.push_front(async { 0 });

    assert_eq!(deque.next().await, Some(0));
    assert_eq!(deque.next().await, Some(1));
    assert_eq!(deque.next().await, Some(2));
    assert_eq!(deque.next().await, None);
});
```

## See also

[API documentation on docs.rs.](https://docs.rs/futurism/latest/futurism/)

Part of the [Folo](https://github.com/folo-rs/folo) project.
