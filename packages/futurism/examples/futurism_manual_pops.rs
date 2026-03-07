//! Demonstrates using manual `pop_front` and `pop_back` to retrieve results
//! from a `LocalFutureDeque` without relying on the `Stream` implementation.

use std::{
    pin::Pin,
    task::{Context, Waker},
};

use futures::Stream;
use futurism::LocalFutureDeque;

fn main() {
    manual_pop_front();
    manual_pop_back();
}

/// Shows how `pop_front` retrieves results after driving the deque.
fn manual_pop_front() {
    let mut deque = LocalFutureDeque::new();

    deque.push_back(async { 10 });
    deque.push_back(async { 20 });
    deque.push_back(async { 30 });

    // Drive all futures by polling the stream once.
    let waker = Waker::noop();
    let cx = &mut Context::from_waker(waker);

    // poll_next drives all futures and consumes the front result (10).
    let poll = Pin::new(&mut deque).poll_next(cx);
    assert_eq!(poll, std::task::Poll::Ready(Some(10)));

    // The remaining futures were also driven and completed. Pop them manually.
    assert_eq!(deque.pop_front(), Some(20));
    assert_eq!(deque.pop_front(), Some(30));
    assert!(deque.is_empty());

    println!("All items popped from front: 10, 20, 30.");
}

/// Shows how `pop_back` retrieves the last completed result.
fn manual_pop_back() {
    let mut deque = LocalFutureDeque::new();

    deque.push_back(async { 100 });
    deque.push_back(async { 200 });
    deque.push_back(async { 300 });

    // Drive all futures.
    let waker = Waker::noop();
    let cx = &mut Context::from_waker(waker);
    let poll = Pin::new(&mut deque).poll_next(cx);
    assert_eq!(poll, std::task::Poll::Ready(Some(100)));

    // Pop from the back instead.
    assert_eq!(deque.pop_back(), Some(300));
    assert_eq!(deque.pop_front(), Some(200));
    assert!(deque.is_empty());

    println!("Popped from back (300), then front (200).");
}
