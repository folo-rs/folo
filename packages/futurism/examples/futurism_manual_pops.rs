//! Demonstrates using manual `pop_front` and `pop_back` to retrieve results
//! from a `LocalFutureDeque` after driving the futures.

use std::task::{Context, Waker};

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

    let waker = Waker::noop();
    let cx = &mut Context::from_waker(waker);

    // Drive all futures.
    deque.drive(cx);

    // Pop all results from front.
    assert_eq!(deque.pop_front(), Some(10));
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

    let waker = Waker::noop();
    let cx = &mut Context::from_waker(waker);

    // Drive all futures.
    deque.drive(cx);

    // Pop from the back first, then front.
    assert_eq!(deque.pop_back(), Some(300));
    assert_eq!(deque.pop_front(), Some(100));
    assert_eq!(deque.pop_front(), Some(200));
    assert!(deque.is_empty());

    println!("Popped from back (300), then front (100, 200).");
}
