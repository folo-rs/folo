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

With the `futures-stream` feature (enabled by default), both types also implement
`futures_core::Stream`, yielding completed results from the front.

## Example

```rust
use std::task::{Context, Poll, Waker};

use futurism::LocalFutureDeque;

let mut deque = LocalFutureDeque::new();

deque.push_back(async { 1 });
deque.push_back(async { 2 });
deque.push_front(async { 0 });

let waker = Waker::noop();
let cx = &mut Context::from_waker(waker);

assert_eq!(deque.poll_front(cx), Poll::Ready(Some(0)));
assert_eq!(deque.poll_front(cx), Poll::Ready(Some(1)));
assert_eq!(deque.poll_front(cx), Poll::Ready(Some(2)));
assert_eq!(deque.poll_front(cx), Poll::Ready(None));
```

## See also

[API documentation on docs.rs.](https://docs.rs/futurism/latest/futurism/)

Part of the [Folo](https://github.com/folo-rs/folo) project.
