//! Example code for the `README.md` file.
//!
//! This contains the same code that appears in the `future_deque` package `README.md`.

use std::task::{Context, Poll, Waker};

use future_deque::LocalFutureDeque;

fn main() {
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
}
