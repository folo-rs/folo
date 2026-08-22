# future_deque design

`future_deque` manages groups of futures while preserving deque ordering for result
retrieval.

## Collection variants

`FutureDeque` is thread-mobile and accepts futures that can move between threads.
`LocalFutureDeque` remains on one thread and accepts futures without that requirement.
Both variants otherwise provide the same polling and retrieval behavior.

## Polling and activation

Active futures are polled in deterministic front-to-back order. A pending future is not
polled again until its waker signals that it may make progress. Waking may occur from any
thread, including for a future stored in the local variant.

The collection becomes ready when every contained future has completed or when it is
empty. New futures may be inserted after readiness and make the collection pending again.

## Result retrieval

Completion does not change deque order. A result can be removed only when the item at the
requested end has completed; completed items behind a pending item remain in place.

The optional stream integration yields completed results from the front and therefore
preserves the same deque semantics.
